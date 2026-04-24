//! Integration tests for the synthetic_exposure program.
//!
//! These tests run against a LiteSVM instance that loads the freshly-built
//! `synthetic_exposure.so` and (for completeness) `mock_pyth.so`. The Pyth
//! price accounts the program consumes are NOT produced via `mock_pyth` —
//! they are seeded directly into LiteSVM with owner set to the real Pyth
//! Receiver program id. This is what lets us drop the old `test-oracle`
//! cargo feature: production and test code paths now parse identically
//! owned account metadata.
//!
//! Mirrors the 8 scenarios from the retired TypeScript suite:
//!   1. initialise a market and store the Pyth feed id
//!   2. long, price rises, close in profit
//!   3. long, price falls, close at a loss (above liquidation)
//!   4. short, price falls, close in profit
//!   5. add collateral to an existing position
//!   6. liquidate an underwater position and pay the bounty
//!   7. reject a position above max leverage
//!   8. reject a stale oracle update

use {
    anchor_lang::{
        solana_program::{instruction::Instruction, pubkey::Pubkey, system_program},
        InstructionData, ToAccountMetas,
    },
    borsh::BorshDeserialize,
    litesvm::LiteSVM,
    solana_account::Account,
    solana_keypair::Keypair,
    solana_kite::{
        create_associated_token_account, create_token_mint, create_wallet,
        get_token_account_balance, mint_tokens_to_token_account,
        send_transaction_from_instructions,
    },
    solana_signer::Signer,
    synthetic_exposure::{
        constants::{ASSET_SYMBOL_LEN, MARKET_SEED, POSITION_SEED, VAULT_SEED},
        oracle::{PRICE_UPDATE_V2_DISCRIMINATOR, PYTH_RECEIVER_PROGRAM_ID},
        state::Side,
    },
};

// ----- constants that mirror the old TypeScript suite ---------------------

// USDC uses 6 decimals — keeping the tests' numerical feel identical to
// the way the protocol is expected to be used in practice.
const QUOTE_DECIMALS: u8 = 6;

// Default market parameters.
const MAINTENANCE_MARGIN_BPS: u16 = 500; // 5%
const MAX_LEVERAGE_BPS: u32 = 100_000; // 10x

// Pyth exponent for every mock price. Real SOL/USD uses -8; we mirror it.
const MOCK_EXPONENT: i32 = -8;

// Scale factor that turns a whole-dollar price into the raw i64 Pyth would
// publish at MOCK_EXPONENT (-8). e.g. $100 -> 100 * 10^8.
const PRICE_SCALE: i64 = 100_000_000;

// PriceUpdateV2 fixed byte length (matches the on-chain layout).
const PRICE_UPDATE_V2_LEN: usize = 134;

// Protocol capital seeded into each market's vault so trader profits have
// somewhere to come from. 10_000 whole USDC units at 6 decimals.
const PROTOCOL_VAULT_CAPITAL: u64 = 10_000 * 1_000_000;

// ----- litesvm setup -----------------------------------------------------

/// Boot a LiteSVM, load both programs, and return a funded payer. The
/// payer is used as the market authority and mint authority throughout.
fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();

    let synthetic_bytes = include_bytes!("../../../target/deploy/synthetic_exposure.so");
    svm.add_program(synthetic_exposure::ID, synthetic_bytes)
        .unwrap();

    // Loaded so the workspace's second program has an on-chain presence
    // even though the tests don't call it. Keeping mock_pyth in the .so
    // graph documents the original Pyth byte layout in executable form
    // alongside the matching parser in `oracle.rs`.
    let mock_pyth_bytes = include_bytes!("../../../target/deploy/mock_pyth.so");
    svm.add_program(mock_pyth::ID, mock_pyth_bytes).unwrap();

    let payer = create_wallet(&mut svm, 100_000_000_000).unwrap();
    (svm, payer)
}

// ----- helpers: PDAs, USDC math, Pyth layout -----------------------------

fn usdc(whole: u64) -> u64 {
    whole * 1_000_000
}

fn scaled_price(dollars: i64) -> i64 {
    dollars * PRICE_SCALE
}

fn encode_asset_symbol(symbol: &str) -> [u8; ASSET_SYMBOL_LEN] {
    let mut bytes = [0u8; ASSET_SYMBOL_LEN];
    let src = symbol.as_bytes();
    assert!(src.len() <= ASSET_SYMBOL_LEN, "symbol too long");
    bytes[..src.len()].copy_from_slice(src);
    bytes
}

/// Fresh, non-colliding 32-byte feed id for each market in the suite.
fn make_feed_id(seed: u8) -> [u8; 32] {
    let mut id = [0u8; 32];
    id[0] = seed;
    for (index, byte) in id.iter_mut().enumerate().skip(1) {
        // Deterministic non-zero pattern so wrong-feed-id bugs surface
        // obviously in error output rather than degrading to "all zeros".
        *byte = ((index as u16).wrapping_mul(17).wrapping_add(seed as u16)) as u8;
    }
    id
}

fn derive_market(symbol: &[u8; ASSET_SYMBOL_LEN]) -> Pubkey {
    let (pda, _bump) =
        Pubkey::find_program_address(&[MARKET_SEED, symbol.as_ref()], &synthetic_exposure::ID);
    pda
}

fn derive_vault(market: &Pubkey) -> Pubkey {
    let (pda, _bump) =
        Pubkey::find_program_address(&[VAULT_SEED, market.as_ref()], &synthetic_exposure::ID);
    pda
}

fn derive_position(market: &Pubkey, owner: &Pubkey) -> Pubkey {
    let (pda, _bump) = Pubkey::find_program_address(
        &[POSITION_SEED, market.as_ref(), owner.as_ref()],
        &synthetic_exposure::ID,
    );
    pda
}

/// Address of the mock price account. The actual mock_pyth PDA derivation
/// is `["mock_price", feed_id]`, but we don't need LiteSVM accounts to
/// live at any particular address — the price_update account is passed as
/// a plain pubkey into every `synthetic_exposure` instruction. Keep the
/// same derivation so the layout stays recognisable across the codebase.
fn derive_price_account(feed_id: &[u8; 32]) -> Pubkey {
    let (pda, _bump) =
        Pubkey::find_program_address(&[b"mock_price", feed_id.as_ref()], &mock_pyth::ID);
    pda
}

/// Current unix timestamp as reported by the LiteSVM clock. Using SVM time
/// (rather than `SystemTime`) keeps publish_time and on-chain `now` in
/// lockstep — otherwise host-clock drift can make a fresh price look stale.
fn svm_now(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<anchor_lang::prelude::Clock>().unix_timestamp
}

/// Build a `PriceUpdateV2` byte payload matching the production layout.
/// Mirrors `mock_pyth::write_price` exactly.
fn build_price_bytes(
    price: i64,
    conf: u64,
    exponent: i32,
    publish_time: i64,
    feed_id: &[u8; 32],
) -> Vec<u8> {
    let mut data = vec![0u8; PRICE_UPDATE_V2_LEN];
    let mut offset = 0usize;
    // 8 bytes — Anchor discriminator.
    data[offset..offset + 8].copy_from_slice(&PRICE_UPDATE_V2_DISCRIMINATOR);
    offset += 8;
    // 32 bytes — write_authority (unused by our reader; zero is fine).
    offset += 32;
    // 2 bytes — VerificationLevel (variant 1 = Full, second byte padding).
    data[offset] = 1;
    data[offset + 1] = 0;
    offset += 2;
    // 32 bytes — feed_id.
    data[offset..offset + 32].copy_from_slice(feed_id);
    offset += 32;
    // 8 bytes — price.
    data[offset..offset + 8].copy_from_slice(&price.to_le_bytes());
    offset += 8;
    // 8 bytes — conf.
    data[offset..offset + 8].copy_from_slice(&conf.to_le_bytes());
    offset += 8;
    // 4 bytes — exponent.
    data[offset..offset + 4].copy_from_slice(&exponent.to_le_bytes());
    offset += 4;
    // 8 bytes — publish_time.
    data[offset..offset + 8].copy_from_slice(&publish_time.to_le_bytes());
    offset += 8;
    // 8 bytes — prev_publish_time (mirror publish_time).
    data[offset..offset + 8].copy_from_slice(&publish_time.to_le_bytes());
    offset += 8;
    // 8 bytes — ema_price (mirror live price).
    data[offset..offset + 8].copy_from_slice(&price.to_le_bytes());
    offset += 8;
    // 8 bytes — ema_conf (mirror live conf).
    data[offset..offset + 8].copy_from_slice(&conf.to_le_bytes());
    // offset += 8; — remaining posted_slot bytes stay zero.
    data
}

/// Seed (or overwrite) a Pyth price account owned by the real Pyth
/// Receiver program id. Returns the account address so the caller can
/// pass it straight into `open_position` / `close_position` / `liquidate`.
fn seed_price_account(
    svm: &mut LiteSVM,
    feed_id: &[u8; 32],
    price: i64,
    conf: u64,
    exponent: i32,
    publish_time: i64,
) -> Pubkey {
    let address = derive_price_account(feed_id);
    let data = build_price_bytes(price, conf, exponent, publish_time, feed_id);
    let rent = svm.minimum_balance_for_rent_exemption(data.len());
    svm.set_account(
        address,
        Account {
            lamports: rent,
            data,
            owner: PYTH_RECEIVER_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    address
}


// ----- helpers: market setup, funding, open/close/liquidate --------------

/// All the addresses a test typically needs to thread through instructions.
struct MarketFixture {
    market: Pubkey,
    vault: Pubkey,
    quote_mint: Pubkey,
    feed_id: [u8; 32],
}

/// Build a fresh market: mint, market PDA, vault PDA, feed id, seeded
/// price account, and protocol-capital topup. Each call uses a unique
/// `feed_seed` so multiple markets can coexist in one test if needed.
fn setup_market(
    svm: &mut LiteSVM,
    authority: &Keypair,
    symbol: &str,
    feed_seed: u8,
    maintenance_margin_bps: u16,
    max_leverage_bps: u32,
) -> MarketFixture {
    let asset_symbol = encode_asset_symbol(symbol);
    let feed_id = make_feed_id(feed_seed);

    let quote_mint = create_token_mint(svm, authority, QUOTE_DECIMALS, None).unwrap();
    let market = derive_market(&asset_symbol);
    let vault = derive_vault(&market);

    let ix = Instruction::new_with_bytes(
        synthetic_exposure::ID,
        &synthetic_exposure::instruction::InitializeMarket {
            asset_symbol,
            pyth_feed_id: feed_id,
            maintenance_margin_bps,
            max_leverage_bps,
        }
        .data(),
        synthetic_exposure::accounts::InitializeMarketAccountConstraints {
            market,
            quote_mint,
            vault,
            authority: authority.pubkey(),
            token_program: spl_token::ID,
            system_program: system_program::id(),
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[authority], &authority.pubkey()).unwrap();

    // Seed the oracle once up-front — individual tests overwrite it with
    // updated prices at the moments that matter. A placeholder $1 price
    // avoids any value ever looking "intentional" if a test forgets to
    // overwrite.
    let publish_time = svm_now(svm);
    seed_price_account(svm, &feed_id, scaled_price(1), 0, MOCK_EXPONENT, publish_time);

    // Top up the vault so trader profits are payable. Minting directly
    // into the vault is safe because the vault PDA is an SPL token
    // account — `mint_to` needs only the mint authority's signature.
    mint_tokens_to_token_account(svm, &quote_mint, &vault, PROTOCOL_VAULT_CAPITAL, authority)
        .unwrap();

    MarketFixture {
        market,
        vault,
        quote_mint,
        feed_id,
    }
}

/// Give a trader an ATA holding `amount` of quote token and return the ATA.
fn fund_trader(
    svm: &mut LiteSVM,
    mint_authority: &Keypair,
    trader: &Pubkey,
    quote_mint: &Pubkey,
    amount: u64,
) -> Pubkey {
    let ata = create_associated_token_account(svm, trader, quote_mint, mint_authority).unwrap();
    if amount > 0 {
        mint_tokens_to_token_account(svm, quote_mint, &ata, amount, mint_authority).unwrap();
    }
    ata
}

/// Open a position. Returns the position PDA.
#[allow(clippy::too_many_arguments)]
fn open_position(
    svm: &mut LiteSVM,
    market: &MarketFixture,
    owner: &Keypair,
    owner_token_account: Pubkey,
    side: Side,
    collateral: u64,
    size: u64,
) -> Pubkey {
    let position = derive_position(&market.market, &owner.pubkey());
    let price_update = derive_price_account(&market.feed_id);

    let ix = Instruction::new_with_bytes(
        synthetic_exposure::ID,
        &synthetic_exposure::instruction::OpenPosition {
            side,
            collateral,
            size,
        }
        .data(),
        synthetic_exposure::accounts::OpenPositionAccountConstraints {
            market: market.market,
            position,
            vault: market.vault,
            owner_token_account,
            quote_mint: market.quote_mint,
            price_update,
            owner: owner.pubkey(),
            token_program: spl_token::ID,
            system_program: system_program::id(),
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[owner], &owner.pubkey()).unwrap();
    position
}

/// Try to open a position without panicking on failure — used for the
/// tests that expect rejection. Returns the raw LiteSVM result.
#[allow(clippy::too_many_arguments)]
fn try_open_position(
    svm: &mut LiteSVM,
    market: &MarketFixture,
    owner: &Keypair,
    owner_token_account: Pubkey,
    side: Side,
    collateral: u64,
    size: u64,
) -> Result<(), String> {
    let position = derive_position(&market.market, &owner.pubkey());
    let price_update = derive_price_account(&market.feed_id);

    let ix = Instruction::new_with_bytes(
        synthetic_exposure::ID,
        &synthetic_exposure::instruction::OpenPosition {
            side,
            collateral,
            size,
        }
        .data(),
        synthetic_exposure::accounts::OpenPositionAccountConstraints {
            market: market.market,
            position,
            vault: market.vault,
            owner_token_account,
            quote_mint: market.quote_mint,
            price_update,
            owner: owner.pubkey(),
            token_program: spl_token::ID,
            system_program: system_program::id(),
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[owner], &owner.pubkey())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

fn close_position(
    svm: &mut LiteSVM,
    market: &MarketFixture,
    owner: &Keypair,
    owner_token_account: Pubkey,
) {
    let position = derive_position(&market.market, &owner.pubkey());
    let price_update = derive_price_account(&market.feed_id);

    let ix = Instruction::new_with_bytes(
        synthetic_exposure::ID,
        &synthetic_exposure::instruction::ClosePosition {}.data(),
        synthetic_exposure::accounts::ClosePositionAccountConstraints {
            market: market.market,
            position,
            vault: market.vault,
            owner_token_account,
            quote_mint: market.quote_mint,
            price_update,
            owner: owner.pubkey(),
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[owner], &owner.pubkey()).unwrap();
}

fn add_collateral(
    svm: &mut LiteSVM,
    market: &MarketFixture,
    owner: &Keypair,
    owner_token_account: Pubkey,
    amount: u64,
) {
    let position = derive_position(&market.market, &owner.pubkey());
    let ix = Instruction::new_with_bytes(
        synthetic_exposure::ID,
        &synthetic_exposure::instruction::AddCollateral { amount }.data(),
        synthetic_exposure::accounts::AddCollateralAccountConstraints {
            market: market.market,
            position,
            vault: market.vault,
            owner_token_account,
            quote_mint: market.quote_mint,
            owner: owner.pubkey(),
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[owner], &owner.pubkey()).unwrap();
}

fn liquidate(
    svm: &mut LiteSVM,
    market: &MarketFixture,
    position_owner: &Pubkey,
    liquidator: &Keypair,
    liquidator_token_account: Pubkey,
) {
    let position = derive_position(&market.market, position_owner);
    let price_update = derive_price_account(&market.feed_id);

    let ix = Instruction::new_with_bytes(
        synthetic_exposure::ID,
        &synthetic_exposure::instruction::Liquidate {}.data(),
        synthetic_exposure::accounts::LiquidateAccountConstraints {
            market: market.market,
            position,
            position_owner: *position_owner,
            vault: market.vault,
            liquidator_token_account,
            quote_mint: market.quote_mint,
            price_update,
            liquidator: liquidator.pubkey(),
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[liquidator], &liquidator.pubkey()).unwrap();
}

// ----- account fetchers --------------------------------------------------

/// Minimal mirror of the on-chain Market layout — same field order as
/// `synthetic_exposure::state::Market` with the 8-byte Anchor
/// discriminator prepended.
#[derive(BorshDeserialize)]
struct MarketSnapshot {
    _discriminator: [u8; 8],
    authority: Pubkey,
    quote_mint: Pubkey,
    vault: Pubkey,
    _vault_bump: u8,
    pyth_feed_id: [u8; 32],
    maintenance_margin_bps: u16,
    max_leverage_bps: u32,
    total_long_size: u64,
    total_short_size: u64,
    asset_symbol: [u8; ASSET_SYMBOL_LEN],
    _bump: u8,
    is_active: bool,
}

fn fetch_market(svm: &LiteSVM, market: &Pubkey) -> MarketSnapshot {
    let account = svm.get_account(market).expect("market exists");
    MarketSnapshot::try_from_slice(&account.data).expect("market deserialises")
}

/// Mirror of `Position`. `Side` is re-used directly from the program crate
/// because its Borsh derive is public.
#[derive(BorshDeserialize)]
struct PositionSnapshot {
    _discriminator: [u8; 8],
    _market: Pubkey,
    _owner: Pubkey,
    side: Side,
    collateral: u64,
    size: u64,
    entry_price: i64,
    entry_price_exponent: i32,
    _opened_at: i64,
    _bump: u8,
}

fn fetch_position(svm: &LiteSVM, position: &Pubkey) -> PositionSnapshot {
    let account = svm.get_account(position).expect("position exists");
    PositionSnapshot::try_from_slice(&account.data).expect("position deserialises")
}


// ----- tests --------------------------------------------------------------

#[test]
fn initialises_a_market_and_stores_the_pyth_feed_id() {
    let (mut svm, authority) = setup();
    let symbol_bytes = encode_asset_symbol("SOL");
    let market = setup_market(
        &mut svm,
        &authority,
        "SOL",
        1,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );

    let snapshot = fetch_market(&svm, &market.market);
    assert_eq!(snapshot.authority, authority.pubkey());
    assert_eq!(snapshot.quote_mint, market.quote_mint);
    assert_eq!(snapshot.vault, market.vault);
    assert_eq!(snapshot.maintenance_margin_bps, MAINTENANCE_MARGIN_BPS);
    assert_eq!(snapshot.max_leverage_bps, MAX_LEVERAGE_BPS);
    assert_eq!(snapshot.total_long_size, 0);
    assert_eq!(snapshot.total_short_size, 0);
    assert!(snapshot.is_active);
    assert_eq!(snapshot.pyth_feed_id, market.feed_id);
    assert_eq!(snapshot.asset_symbol, symbol_bytes);
}

#[test]
fn opens_a_long_price_rises_closes_in_profit() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "BTC",
        2,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );

    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();

    // Price starts at $100. size=50 USDC, collateral=10 USDC → 5x
    // leverage. Price then moves to $150 (+50%). PnL = 50 * 0.5 = 25 USDC.
    // Payout = 10 + 25 = 35.
    let open_price = scaled_price(100);
    let close_price = scaled_price(150);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, open_price, 0, MOCK_EXPONENT, now);

    let collateral = usdc(10);
    let size = usdc(50);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        collateral,
    );

    let position_address =
        open_position(&mut svm, &market, &trader, trader_quote, Side::Long, collateral, size);

    let position = fetch_position(&svm, &position_address);
    assert_eq!(position.side, Side::Long);
    assert_eq!(position.collateral, collateral);
    assert_eq!(position.size, size);
    assert_eq!(position.entry_price, open_price);
    assert_eq!(position.entry_price_exponent, MOCK_EXPONENT);

    // Advance svm time a touch so the new oracle publish_time is strictly
    // greater than the old — not strictly required but matches what a live
    // Pyth feed would look like between two reads.
    svm.expire_blockhash();
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, close_price, 0, MOCK_EXPONENT, now);
    close_position(&mut svm, &market, &trader, trader_quote);

    assert_eq!(get_token_account_balance(&svm, &trader_quote).unwrap(), usdc(35));
}

#[test]
fn opens_a_long_price_falls_closes_at_a_loss_above_liquidation() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "ETH",
        3,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );
    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();

    // collateral=20, size=40 → 2x leverage. 5% drop: pnl = 40 * -0.05 = -2
    // → payout = 18. Equity 18 vs maintenance = 40 * 5% = 2 → healthy.
    let open_price = scaled_price(100);
    let close_price = scaled_price(95);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, open_price, 0, MOCK_EXPONENT, now);

    let collateral = usdc(20);
    let size = usdc(40);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        collateral,
    );
    open_position(&mut svm, &market, &trader, trader_quote, Side::Long, collateral, size);

    svm.expire_blockhash();
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, close_price, 0, MOCK_EXPONENT, now);
    close_position(&mut svm, &market, &trader, trader_quote);

    assert_eq!(get_token_account_balance(&svm, &trader_quote).unwrap(), usdc(18));
}

#[test]
fn opens_a_short_price_falls_closes_in_profit() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "DOGE",
        4,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );
    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();

    // Short: entry $200 → close $150 (-25%). size=20, collateral=10.
    // pnl = 20 * (200-150)/200 = 5 USDC. payout = 15.
    let open_price = scaled_price(200);
    let close_price = scaled_price(150);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, open_price, 0, MOCK_EXPONENT, now);

    let collateral = usdc(10);
    let size = usdc(20);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        collateral,
    );
    open_position(&mut svm, &market, &trader, trader_quote, Side::Short, collateral, size);

    svm.expire_blockhash();
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, close_price, 0, MOCK_EXPONENT, now);
    close_position(&mut svm, &market, &trader, trader_quote);

    assert_eq!(get_token_account_balance(&svm, &trader_quote).unwrap(), usdc(15));
}

#[test]
fn adds_collateral_to_an_existing_position() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "LINK",
        5,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );
    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let open_price = scaled_price(10);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, open_price, 0, MOCK_EXPONENT, now);

    let initial_collateral = usdc(5);
    let top_up = usdc(3);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        initial_collateral + top_up,
    );

    let position_address = open_position(
        &mut svm,
        &market,
        &trader,
        trader_quote,
        Side::Long,
        initial_collateral,
        usdc(10),
    );

    add_collateral(&mut svm, &market, &trader, trader_quote, top_up);

    let position = fetch_position(&svm, &position_address);
    assert_eq!(position.collateral, initial_collateral + top_up);

    // Vault holds the seeded protocol capital plus every collateral
    // contribution made so far.
    let vault_balance = get_token_account_balance(&svm, &market.vault).unwrap();
    assert_eq!(vault_balance, PROTOCOL_VAULT_CAPITAL + initial_collateral + top_up);
}

#[test]
fn liquidates_an_underwater_position_and_pays_the_bounty() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "ADA",
        6,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );
    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let liquidator = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let open_price = scaled_price(100);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, open_price, 0, MOCK_EXPONENT, now);

    // 10x leverage: collateral=10, size=100. A 6% drop gives pnl = -6,
    // equity = 4, maintenance = 100 * 5% = 5. equity < maintenance →
    // liquidatable. Bounty = 5% of 4 = 0.2 USDC = 200_000 atoms.
    let collateral = usdc(10);
    let size = usdc(100);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        collateral,
    );
    let liquidator_quote = fund_trader(
        &mut svm,
        &authority,
        &liquidator.pubkey(),
        &market.quote_mint,
        0,
    );

    open_position(&mut svm, &market, &trader, trader_quote, Side::Long, collateral, size);

    svm.expire_blockhash();
    let distressed_price = scaled_price(94);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, distressed_price, 0, MOCK_EXPONENT, now);

    liquidate(&mut svm, &market, &trader.pubkey(), &liquidator, liquidator_quote);

    assert_eq!(get_token_account_balance(&svm, &liquidator_quote).unwrap(), 200_000);
}

#[test]
fn rejects_opening_a_position_above_max_leverage() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "XRP",
        7,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );
    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let now = svm_now(&svm);
    seed_price_account(&mut svm, &market.feed_id, scaled_price(100), 0, MOCK_EXPONENT, now);

    // MAX_LEVERAGE_BPS = 100_000 = 10x. 11x should fail.
    let collateral = usdc(10);
    let size = usdc(110);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        collateral,
    );

    let err =
        try_open_position(&mut svm, &market, &trader, trader_quote, Side::Long, collateral, size)
            .expect_err("above-max-leverage open should fail");

    // InvalidLeverage = first ErrorCode variant → code 6000 (0x1770).
    assert!(
        err.contains("6000") || err.contains("0x1770") || err.contains("InvalidLeverage"),
        "expected InvalidLeverage / 0x1770 / 6000 in error, got: {err}"
    );
}

#[test]
fn rejects_a_stale_oracle_update() {
    let (mut svm, authority) = setup();
    let market = setup_market(
        &mut svm,
        &authority,
        "AVAX",
        8,
        MAINTENANCE_MARGIN_BPS,
        MAX_LEVERAGE_BPS,
    );
    let trader = create_wallet(&mut svm, 1_000_000_000).unwrap();

    // publish_time 10 minutes in the past vs. STALENESS_MAX_SECONDS = 60.
    let now = svm_now(&svm);
    let stale_publish_time = now - 600;
    seed_price_account(
        &mut svm,
        &market.feed_id,
        scaled_price(20),
        0,
        MOCK_EXPONENT,
        stale_publish_time,
    );

    let collateral = usdc(10);
    let trader_quote = fund_trader(
        &mut svm,
        &authority,
        &trader.pubkey(),
        &market.quote_mint,
        collateral,
    );

    let err = try_open_position(
        &mut svm,
        &market,
        &trader,
        trader_quote,
        Side::Long,
        collateral,
        usdc(20),
    )
    .expect_err("stale oracle should reject the open");

    // OracleStale = 4th variant (index 3) → code 6003 (0x1773).
    assert!(
        err.contains("6003") || err.contains("0x1773") || err.contains("OracleStale"),
        "expected OracleStale / 0x1773 / 6003 in error, got: {err}"
    );
}
