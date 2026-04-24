//! Integration tests for the put_swap protective-put primitive.
//!
//! Each test boots a fresh LiteSVM, loads the compiled put_swap
//! and mock_pyth .so files, seeds a Pyth-owned price account directly via
//! `LiteSVM::set_account`, and drives a full lifecycle from create to
//! settle / cancel / liquidate.
//!
//! Production and test code paths read identical account metadata — no
//! feature flags branch between them. That's why the oracle owner check
//! stays strict in prod and still passes here.

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
    put_swap::{
        constants::{ASSET_VAULT_SEED, COLLATERAL_VAULT_SEED, SWAP_SEED},
        oracle::{PRICE_UPDATE_V2_DISCRIMINATOR, PYTH_RECEIVER_PROGRAM_ID},
        state::SwapStatus,
    },
};

// ----------- constants ---------------------------------------------------

// USDC-style quote mint — 6 decimals.
const QUOTE_DECIMALS: u8 = 6;

// Asset mint decimals (picked to match SOL's 9-decimal convention so the
// tests exercise the decimal-adjustment path in `compute_notional_quote`).
const ASSET_DECIMALS: u8 = 9;

// Pyth exponent used for every mock price. Real SOL/USD uses -8.
const MOCK_EXPONENT: i32 = -8;

// Raw-price scale factor: a whole-dollar price `d` is encoded as
// `d * 1e8` at exponent -8.
const PRICE_SCALE: i64 = 100_000_000;

// `PriceUpdateV2` fixed byte length.
const PRICE_UPDATE_V2_LEN: usize = 134;

// ----------- litesvm boot ------------------------------------------------

fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();

    let put_swap_bytes = include_bytes!("../../../target/deploy/put_swap.so");
    svm.add_program(put_swap::ID, put_swap_bytes).unwrap();

    // Keep mock_pyth loaded so the workspace's second program has an
    // on-chain presence even though we don't CPI into it — it serves as
    // executable documentation of the `PriceUpdateV2` byte layout.
    let mock_pyth_bytes = include_bytes!("../../../target/deploy/mock_pyth.so");
    svm.add_program(mock_pyth::ID, mock_pyth_bytes).unwrap();

    let payer = create_wallet(&mut svm, 100_000_000_000).unwrap();
    (svm, payer)
}

// ----------- helpers: PDAs, amounts, oracle seeding ----------------------

fn whole_asset(units: u64) -> u64 {
    units.checked_mul(10u64.pow(ASSET_DECIMALS as u32)).unwrap()
}

fn usdc(whole: u64) -> u64 {
    whole.checked_mul(10u64.pow(QUOTE_DECIMALS as u32)).unwrap()
}

fn scaled_price(dollars: i64) -> i64 {
    dollars * PRICE_SCALE
}

fn make_feed_id(seed: u8) -> [u8; 32] {
    let mut id = [0u8; 32];
    id[0] = seed;
    for (index, byte) in id.iter_mut().enumerate().skip(1) {
        *byte = ((index as u16).wrapping_mul(17).wrapping_add(seed as u16)) as u8;
    }
    id
}

fn derive_swap(party_a: &Pubkey, swap_id_seed: &[u8; 8]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SWAP_SEED, party_a.as_ref(), swap_id_seed.as_ref()],
        &put_swap::ID,
    )
}

fn derive_asset_vault(swap: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[ASSET_VAULT_SEED, swap.as_ref()],
        &put_swap::ID,
    )
    .0
}

fn derive_collateral_vault(swap: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[COLLATERAL_VAULT_SEED, swap.as_ref()],
        &put_swap::ID,
    )
    .0
}

/// Address used for the mock Pyth price account. Any stable pubkey works
/// here; we use the `mock_pyth` PDA derivation so the address is
/// recognisable in logs. `LiteSVM::set_account` lets us override the owner.
fn derive_price_account(feed_id: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(&[b"mock_price", feed_id.as_ref()], &mock_pyth::ID).0
}

fn svm_now(svm: &LiteSVM) -> i64 {
    svm.get_sysvar::<anchor_lang::prelude::Clock>().unix_timestamp
}

/// Overwrite the SVM clock to move time forward. Used to reach expiry
/// without having to publish hundreds of blocks.
fn advance_time(svm: &mut LiteSVM, to_ts: i64) {
    let mut clock = svm.get_sysvar::<anchor_lang::prelude::Clock>();
    clock.unix_timestamp = to_ts;
    svm.set_sysvar::<anchor_lang::prelude::Clock>(&clock);
}

fn build_price_bytes(
    price: i64,
    conf: u64,
    exponent: i32,
    publish_time: i64,
    feed_id: &[u8; 32],
) -> Vec<u8> {
    let mut data = vec![0u8; PRICE_UPDATE_V2_LEN];
    let mut offset = 0usize;
    data[offset..offset + 8].copy_from_slice(&PRICE_UPDATE_V2_DISCRIMINATOR);
    offset += 8;
    offset += 32; // write_authority
    data[offset] = 1; // VerificationLevel::Full
    data[offset + 1] = 0;
    offset += 2;
    data[offset..offset + 32].copy_from_slice(feed_id);
    offset += 32;
    data[offset..offset + 8].copy_from_slice(&price.to_le_bytes());
    offset += 8;
    data[offset..offset + 8].copy_from_slice(&conf.to_le_bytes());
    offset += 8;
    data[offset..offset + 4].copy_from_slice(&exponent.to_le_bytes());
    offset += 4;
    data[offset..offset + 8].copy_from_slice(&publish_time.to_le_bytes());
    offset += 8;
    data[offset..offset + 8].copy_from_slice(&publish_time.to_le_bytes()); // prev_publish_time
    offset += 8;
    data[offset..offset + 8].copy_from_slice(&price.to_le_bytes()); // ema_price
    offset += 8;
    data[offset..offset + 8].copy_from_slice(&conf.to_le_bytes()); // ema_conf
    data
}

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

// ----------- fixtures ----------------------------------------------------

struct SwapFixture {
    swap: Pubkey,
    asset_vault: Pubkey,
    collateral_vault: Pubkey,
    asset_mint: Pubkey,
    quote_mint: Pubkey,
    feed_id: [u8; 32],
    // Retained for debuggability (the PDA seeds are derivable without it,
    // but having the raw bytes nearby makes log inspection easier).
    #[allow(dead_code)]
    swap_id_seed: [u8; 8],
}

/// Build two SPL mints (asset + quote), issue a fresh feed id, and seed an
/// initial oracle price. Each call uses a unique `feed_seed` so multiple
/// tests in the same suite can't collide on price accounts or mints.
fn setup_market(
    svm: &mut LiteSVM,
    authority: &Keypair,
    feed_seed: u8,
    initial_price_usd: i64,
) -> (Pubkey, Pubkey, [u8; 32]) {
    let asset_mint = create_token_mint(svm, authority, ASSET_DECIMALS, None).unwrap();
    let quote_mint = create_token_mint(svm, authority, QUOTE_DECIMALS, None).unwrap();
    let feed_id = make_feed_id(feed_seed);
    let now = svm_now(svm);
    seed_price_account(svm, &feed_id, scaled_price(initial_price_usd), 0, MOCK_EXPONENT, now);
    (asset_mint, quote_mint, feed_id)
}

/// Mint an ATA for `wallet` in `mint` and preload `amount` atoms.
fn fund(
    svm: &mut LiteSVM,
    mint_authority: &Keypair,
    wallet: &Pubkey,
    mint: &Pubkey,
    amount: u64,
) -> Pubkey {
    let ata = create_associated_token_account(svm, wallet, mint, mint_authority).unwrap();
    if amount > 0 {
        mint_tokens_to_token_account(svm, mint, &ata, amount, mint_authority).unwrap();
    }
    ata
}

// ----------- instruction wrappers ----------------------------------------

#[allow(clippy::too_many_arguments)]
fn ix_create_swap(
    svm: &mut LiteSVM,
    party_a: &Keypair,
    party_a_asset: Pubkey,
    party_a_quote: Pubkey,
    asset_mint: Pubkey,
    quote_mint: Pubkey,
    feed_id: [u8; 32],
    swap_id_seed: [u8; 8],
    amount_asset: u64,
    required_collateral: u64,
    premium: u64,
    expiry_ts: i64,
    fill_deadline_ts: i64,
) -> SwapFixture {
    let (swap, _bump) = derive_swap(&party_a.pubkey(), &swap_id_seed);
    let asset_vault = derive_asset_vault(&swap);
    let collateral_vault = derive_collateral_vault(&swap);
    let price_update = derive_price_account(&feed_id);

    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::CreateSwap {
            swap_id_seed,
            amount_asset,
            required_collateral,
            premium,
            expiry_ts,
            fill_deadline_ts,
            pyth_feed_id: feed_id,
        }
        .data(),
        put_swap::accounts::CreateSwapAccountConstraints {
            swap,
            asset_mint,
            quote_mint,
            asset_token_program: spl_token::ID,
            quote_token_program: spl_token::ID,
            asset_vault,
            collateral_vault,
            party_a_asset_account: party_a_asset,
            party_a_quote_account: party_a_quote,
            price_update,
            party_a: party_a.pubkey(),
            system_program: system_program::id(),
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[party_a], &party_a.pubkey()).unwrap();

    SwapFixture {
        swap,
        asset_vault,
        collateral_vault,
        asset_mint,
        quote_mint,
        feed_id,
        swap_id_seed,
    }
}

fn ix_fill_swap(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    party_b: &Keypair,
    party_b_quote: Pubkey,
    collateral_amount: u64,
) {
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::FillSwap { collateral_amount }.data(),
        put_swap::accounts::FillSwapAccountConstraints {
            swap: fix.swap,
            collateral_vault: fix.collateral_vault,
            party_b_quote_account: party_b_quote,
            quote_mint: fix.quote_mint,
            party_b: party_b.pubkey(),
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[party_b], &party_b.pubkey()).unwrap();
}

fn try_ix_fill_swap(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    party_b: &Keypair,
    party_b_quote: Pubkey,
    collateral_amount: u64,
) -> Result<(), String> {
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::FillSwap { collateral_amount }.data(),
        put_swap::accounts::FillSwapAccountConstraints {
            swap: fix.swap,
            collateral_vault: fix.collateral_vault,
            party_b_quote_account: party_b_quote,
            quote_mint: fix.quote_mint,
            party_b: party_b.pubkey(),
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[party_b], &party_b.pubkey())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

fn ix_add_collateral(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    party_b: &Keypair,
    party_b_quote: Pubkey,
    amount: u64,
) {
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::AddCollateral { amount }.data(),
        put_swap::accounts::AddCollateralAccountConstraints {
            swap: fix.swap,
            collateral_vault: fix.collateral_vault,
            party_b_quote_account: party_b_quote,
            quote_mint: fix.quote_mint,
            party_b: party_b.pubkey(),
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[party_b], &party_b.pubkey()).unwrap();
}

fn ix_cancel_swap(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    party_a: &Keypair,
    party_a_asset: Pubkey,
    party_a_quote: Pubkey,
) {
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::CancelSwap {}.data(),
        put_swap::accounts::CancelSwapAccountConstraints {
            swap: fix.swap,
            asset_vault: fix.asset_vault,
            collateral_vault: fix.collateral_vault,
            party_a_asset_account: party_a_asset,
            party_a_quote_account: party_a_quote,
            asset_mint: fix.asset_mint,
            quote_mint: fix.quote_mint,
            party_a: party_a.pubkey(),
            asset_token_program: spl_token::ID,
            quote_token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[party_a], &party_a.pubkey()).unwrap();
}

fn try_ix_cancel_swap(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    party_a: &Keypair,
    party_a_asset: Pubkey,
    party_a_quote: Pubkey,
) -> Result<(), String> {
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::CancelSwap {}.data(),
        put_swap::accounts::CancelSwapAccountConstraints {
            swap: fix.swap,
            asset_vault: fix.asset_vault,
            collateral_vault: fix.collateral_vault,
            party_a_asset_account: party_a_asset,
            party_a_quote_account: party_a_quote,
            asset_mint: fix.asset_mint,
            quote_mint: fix.quote_mint,
            party_a: party_a.pubkey(),
            asset_token_program: spl_token::ID,
            quote_token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[party_a], &party_a.pubkey())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[allow(clippy::too_many_arguments)]
fn ix_settle_swap(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    caller: &Keypair,
    party_a_asset: Pubkey,
    party_a_quote: Pubkey,
    party_b_quote: Pubkey,
) {
    let price_update = derive_price_account(&fix.feed_id);
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::SettleSwap {}.data(),
        put_swap::accounts::SettleSwapAccountConstraints {
            swap: fix.swap,
            asset_vault: fix.asset_vault,
            collateral_vault: fix.collateral_vault,
            party_a_asset_account: party_a_asset,
            party_a_quote_account: party_a_quote,
            party_b_quote_account: party_b_quote,
            asset_mint: fix.asset_mint,
            quote_mint: fix.quote_mint,
            price_update,
            caller: caller.pubkey(),
            asset_token_program: spl_token::ID,
            quote_token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[caller], &caller.pubkey()).unwrap();
}

fn try_ix_settle_swap(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    caller: &Keypair,
    party_a_asset: Pubkey,
    party_a_quote: Pubkey,
    party_b_quote: Pubkey,
) -> Result<(), String> {
    let price_update = derive_price_account(&fix.feed_id);
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::SettleSwap {}.data(),
        put_swap::accounts::SettleSwapAccountConstraints {
            swap: fix.swap,
            asset_vault: fix.asset_vault,
            collateral_vault: fix.collateral_vault,
            party_a_asset_account: party_a_asset,
            party_a_quote_account: party_a_quote,
            party_b_quote_account: party_b_quote,
            asset_mint: fix.asset_mint,
            quote_mint: fix.quote_mint,
            price_update,
            caller: caller.pubkey(),
            asset_token_program: spl_token::ID,
            quote_token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[caller], &caller.pubkey())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[allow(clippy::too_many_arguments)]
fn ix_liquidate(
    svm: &mut LiteSVM,
    fix: &SwapFixture,
    liquidator: &Keypair,
    liquidator_quote: Pubkey,
    party_a_asset: Pubkey,
    party_a_quote: Pubkey,
    party_b_quote: Pubkey,
) {
    let price_update = derive_price_account(&fix.feed_id);
    let ix = Instruction::new_with_bytes(
        put_swap::ID,
        &put_swap::instruction::Liquidate {}.data(),
        put_swap::accounts::LiquidateAccountConstraints {
            swap: fix.swap,
            asset_vault: fix.asset_vault,
            collateral_vault: fix.collateral_vault,
            party_a_asset_account: party_a_asset,
            party_a_quote_account: party_a_quote,
            party_b_quote_account: party_b_quote,
            liquidator_quote_account: liquidator_quote,
            asset_mint: fix.asset_mint,
            quote_mint: fix.quote_mint,
            price_update,
            liquidator: liquidator.pubkey(),
            asset_token_program: spl_token::ID,
            quote_token_program: spl_token::ID,
        }
        .to_account_metas(None),
    );
    send_transaction_from_instructions(svm, vec![ix], &[liquidator], &liquidator.pubkey())
        .unwrap();
}

// ----------- account fetchers --------------------------------------------

#[derive(BorshDeserialize)]
struct SwapSnapshot {
    _discriminator: [u8; 8],
    _party_a: Pubkey,
    party_b: Option<Pubkey>,
    _asset_mint: Pubkey,
    _quote_mint: Pubkey,
    _asset_vault: Pubkey,
    _collateral_vault: Pubkey,
    amount_asset: u64,
    _entry_price_raw: i64,
    _entry_price_exponent: i32,
    notional_quote: u64,
    _required_collateral: u64,
    collateral_posted: u64,
    _premium: u64,
    _expiry_ts: i64,
    _fill_deadline_ts: i64,
    _pyth_feed_id: [u8; 32],
    _swap_id_seed: [u8; 8],
    status: SwapStatus,
    _bump: u8,
    _asset_vault_bump: u8,
    _collateral_vault_bump: u8,
}

fn fetch_swap(svm: &LiteSVM, swap: &Pubkey) -> SwapSnapshot {
    let account = svm.get_account(swap).expect("swap exists");
    // `try_from_slice` refuses leftover bytes, but Anchor's `InitSpace`
    // allocates the MAX serialised size — `Option::None` only consumes
    // one byte instead of `1 + size_of::<Pubkey>()`, leaving trailing
    // zero padding. `deserialize_reader` happily stops at EOF of its
    // parsed fields so we use that instead.
    let mut data = &account.data[..];
    SwapSnapshot::deserialize_reader(&mut data).expect("swap deserialises")
}

// ----------- tests -------------------------------------------------------

// Standardised numbers used across the scenario tests to keep the math in
// one place: 1 whole asset unit at $100 ⇒ notional = 100 USDC =
// 100_000_000 quote atoms. At 30% initial margin the minimum collateral
// is 30 USDC; premium of 1 USDC (1% of notional) sits comfortably below
// the 5% MAX_PREMIUM_BPS ceiling.
const TEST_ASSET_AMOUNT: u64 = 1; // whole asset units before decimals
const TEST_ENTRY_PRICE: i64 = 100;
const TEST_COLLATERAL: u64 = 30; // whole USDC
const TEST_PREMIUM: u64 = 1; // whole USDC

fn standard_swap_times(svm: &LiteSVM) -> (i64, i64) {
    // Fill deadline 1 hour out, expiry 2 hours. Plenty of room for tests
    // that want to fill, top up, then warp forward to expiry.
    let now = svm_now(svm);
    (now + 3_600, now + 7_200)
}

#[test]
fn creates_a_swap_and_locks_the_asset() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 1, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [1u8; 8],
        asset_amount,
        usdc(TEST_COLLATERAL),
        premium_atoms,
        expiry,
        fill_deadline,
    );

    // Asset escrowed in the asset vault; A's ATA drained.
    assert_eq!(get_token_account_balance(&svm, &fix.asset_vault).unwrap(), asset_amount);
    assert_eq!(get_token_account_balance(&svm, &a_asset).unwrap(), 0);

    // Fee pre-funded into the collateral vault.
    assert_eq!(get_token_account_balance(&svm, &fix.collateral_vault).unwrap(), premium_atoms);

    let snapshot = fetch_swap(&svm, &fix.swap);
    assert_eq!(snapshot.status, SwapStatus::Created);
    assert!(snapshot.party_b.is_none());
    assert_eq!(snapshot.amount_asset, asset_amount);
    assert_eq!(snapshot.notional_quote, usdc(100)); // 1 SOL × $100 = 100 USDC
    assert_eq!(snapshot.collateral_posted, 0);
}

#[test]
fn fills_a_swap_and_transfers_premium() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 2, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [2u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );

    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // After fill: collateral vault holds exactly the posted collateral
    // (fee has been swept out to B). B's ATA holds only the fee (we
    // pre-funded exactly the collateral, no excess).
    assert_eq!(
        get_token_account_balance(&svm, &fix.collateral_vault).unwrap(),
        collateral_atoms
    );
    assert_eq!(get_token_account_balance(&svm, &b_quote).unwrap(), premium_atoms);

    let snapshot = fetch_swap(&svm, &fix.swap);
    assert_eq!(snapshot.status, SwapStatus::Active);
    assert_eq!(snapshot.party_b, Some(party_b.pubkey()));
    assert_eq!(snapshot.collateral_posted, collateral_atoms);
}

#[test]
fn cancels_an_unfilled_swap_and_refunds_party_a() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 3, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [3u8; 8],
        asset_amount,
        usdc(TEST_COLLATERAL),
        premium_atoms,
        expiry,
        fill_deadline,
    );

    ix_cancel_swap(&mut svm, &fix, &party_a, a_asset, a_quote);

    // A fully refunded: asset back, fee back, vaults empty.
    assert_eq!(get_token_account_balance(&svm, &a_asset).unwrap(), asset_amount);
    assert_eq!(get_token_account_balance(&svm, &a_quote).unwrap(), premium_atoms);
    assert_eq!(get_token_account_balance(&svm, &fix.asset_vault).unwrap(), 0);
    assert_eq!(get_token_account_balance(&svm, &fix.collateral_vault).unwrap(), 0);

    assert_eq!(fetch_swap(&svm, &fix.swap).status, SwapStatus::Cancelled);
}

#[test]
fn settles_with_price_appreciation_party_b_wins() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 4, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [4u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Price rises to $120 (+20%). B's PnL = +20 USDC. Party A kept the
    // (now more valuable) asset — that's A's upside compensation; A
    // receives nothing from the collateral vault. B keeps their full
    // collateral.
    advance_time(&mut svm, expiry + 1);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(120), 0, MOCK_EXPONENT, now);

    ix_settle_swap(&mut svm, &fix, &party_a, a_asset, a_quote, b_quote);

    // Asset back to A in full, A's quote untouched (still holds the fee
    // they paid to fund; fee was transferred to B at fill).
    assert_eq!(get_token_account_balance(&svm, &a_asset).unwrap(), asset_amount);
    assert_eq!(get_token_account_balance(&svm, &a_quote).unwrap(), 0);
    // B got their collateral + the fee they already received at fill.
    assert_eq!(
        get_token_account_balance(&svm, &b_quote).unwrap(),
        collateral_atoms + premium_atoms
    );
    assert_eq!(fetch_swap(&svm, &fix.swap).status, SwapStatus::Settled);
}

#[test]
fn settles_with_price_depreciation_party_a_wins() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 5, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [5u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Price falls to $80 (-20%). B's PnL = -20 USDC. A claims 20 USDC
    // from the collateral vault; B keeps 10.
    advance_time(&mut svm, expiry + 1);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(80), 0, MOCK_EXPONENT, now);

    ix_settle_swap(&mut svm, &fix, &party_a, a_asset, a_quote, b_quote);

    assert_eq!(get_token_account_balance(&svm, &a_asset).unwrap(), asset_amount);
    assert_eq!(get_token_account_balance(&svm, &a_quote).unwrap(), usdc(20));
    // B keeps the remaining 10 USDC plus the fee they received at fill.
    assert_eq!(
        get_token_account_balance(&svm, &b_quote).unwrap(),
        usdc(10) + premium_atoms
    );
    assert_eq!(fetch_swap(&svm, &fix.swap).status, SwapStatus::Settled);
}

#[test]
fn settles_with_party_b_wiped_out_exactly() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 6, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [6u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Price falls to $70 (-30%). B's PnL = -30 USDC == entire collateral.
    // A gets the full collateral, B gets 0 (plus the fee they already
    // received at fill).
    advance_time(&mut svm, expiry + 1);
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(70), 0, MOCK_EXPONENT, now);

    ix_settle_swap(&mut svm, &fix, &party_a, a_asset, a_quote, b_quote);

    assert_eq!(get_token_account_balance(&svm, &a_asset).unwrap(), asset_amount);
    assert_eq!(get_token_account_balance(&svm, &a_quote).unwrap(), collateral_atoms);
    assert_eq!(get_token_account_balance(&svm, &b_quote).unwrap(), premium_atoms);
    assert_eq!(get_token_account_balance(&svm, &fix.collateral_vault).unwrap(), 0);
}

#[test]
fn liquidates_when_party_b_goes_underwater_mid_term() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 7, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let liquidator = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);
    let liquidator_quote = fund(&mut svm, &authority, &liquidator.pubkey(), &quote_mint, 0);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [7u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Price crashes to $75 (-25%). PnL_B = -25 USDC → equity = 30 - 25 = 5.
    // Maintenance = 10% of 100 USDC notional = 10. 5 < 10 → liquidatable.
    // Still within fill window — no need to advance time.
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(75), 0, MOCK_EXPONENT, now);

    ix_liquidate(
        &mut svm,
        &fix,
        &liquidator,
        liquidator_quote,
        a_asset,
        a_quote,
        b_quote,
    );

    // Bounty = 5% of A's share (25 USDC) = 1.25 USDC = 1_250_000 atoms.
    assert_eq!(
        get_token_account_balance(&svm, &liquidator_quote).unwrap(),
        1_250_000
    );
    // A's remainder after bounty = 25 - 1.25 = 23.75 USDC = 23_750_000.
    assert_eq!(
        get_token_account_balance(&svm, &a_quote).unwrap(),
        23_750_000
    );
    // B keeps the residual 5 USDC plus the fee they already received.
    assert_eq!(
        get_token_account_balance(&svm, &b_quote).unwrap(),
        usdc(5) + premium_atoms
    );
    assert_eq!(get_token_account_balance(&svm, &a_asset).unwrap(), asset_amount);
    assert_eq!(fetch_swap(&svm, &fix.swap).status, SwapStatus::Settled);
}

#[test]
fn adding_collateral_restores_health_and_blocks_liquidation() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 8, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let liquidator = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);
    let top_up = usdc(20);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(
        &mut svm,
        &authority,
        &party_b.pubkey(),
        &quote_mint,
        collateral_atoms + top_up,
    );
    let liquidator_quote = fund(&mut svm, &authority, &liquidator.pubkey(), &quote_mint, 0);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [8u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Top up: collateral goes from 30 to 50 USDC.
    ix_add_collateral(&mut svm, &fix, &party_b, b_quote, top_up);
    assert_eq!(
        fetch_swap(&svm, &fix.swap).collateral_posted,
        collateral_atoms + top_up
    );

    // Same 25% crash that liquidated previously now leaves equity =
    // 50 - 25 = 25 > 10 maintenance → NOT liquidatable. Expect attempt
    // to fail.
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(75), 0, MOCK_EXPONENT, now);

    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ix_liquidate(
            &mut svm,
            &fix,
            &liquidator,
            liquidator_quote,
            a_asset,
            a_quote,
            b_quote,
        );
    }));
    assert!(err.is_err(), "liquidate on a healthy position should fail");
}

#[test]
fn rejects_fill_with_insufficient_collateral() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) = setup_market(&mut svm, &authority, 9, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [9u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );

    // Offer only 29 USDC when 30 is required.
    let err = try_ix_fill_swap(&mut svm, &fix, &party_b, b_quote, usdc(29))
        .expect_err("fill with sub-required collateral must fail");
    assert!(
        err.contains("InsufficientCollateral") || err.contains("6003") || err.contains("0x1773"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_settle_before_expiry() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) =
        setup_market(&mut svm, &authority, 10, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [10u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Still before expiry: refresh the oracle to the same price and
    // attempt settlement. Expect NotYetExpired.
    let now = svm_now(&svm);
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(100), 0, MOCK_EXPONENT, now);

    let err = try_ix_settle_swap(&mut svm, &fix, &party_a, a_asset, a_quote, b_quote)
        .expect_err("settle before expiry must fail");
    assert!(
        err.contains("NotYetExpired") || err.contains("6008") || err.contains("0x1778"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_stale_oracle_at_settle() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) =
        setup_market(&mut svm, &authority, 11, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [11u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    // Warp to after expiry, then seed a price whose publish_time is
    // >60 seconds (STALENESS_MAX_SECONDS) before the new clock.
    advance_time(&mut svm, expiry + 120);
    let stale_ts = svm_now(&svm) - 600;
    seed_price_account(&mut svm, &fix.feed_id, scaled_price(100), 0, MOCK_EXPONENT, stale_ts);

    let err = try_ix_settle_swap(&mut svm, &fix, &party_a, a_asset, a_quote, b_quote)
        .expect_err("stale oracle must reject settle");
    assert!(
        err.contains("OracleStale") || err.contains("6010") || err.contains("0x177a"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_cancel_after_swap_filled() {
    let (mut svm, authority) = setup();
    let (asset_mint, quote_mint, feed_id) =
        setup_market(&mut svm, &authority, 12, TEST_ENTRY_PRICE);

    let party_a = create_wallet(&mut svm, 1_000_000_000).unwrap();
    let party_b = create_wallet(&mut svm, 1_000_000_000).unwrap();

    let asset_amount = whole_asset(TEST_ASSET_AMOUNT);
    let premium_atoms = usdc(TEST_PREMIUM);
    let collateral_atoms = usdc(TEST_COLLATERAL);

    let a_asset = fund(&mut svm, &authority, &party_a.pubkey(), &asset_mint, asset_amount);
    let a_quote = fund(&mut svm, &authority, &party_a.pubkey(), &quote_mint, premium_atoms);
    let b_quote = fund(&mut svm, &authority, &party_b.pubkey(), &quote_mint, collateral_atoms);

    let (fill_deadline, expiry) = standard_swap_times(&svm);
    let fix = ix_create_swap(
        &mut svm,
        &party_a,
        a_asset,
        a_quote,
        asset_mint,
        quote_mint,
        feed_id,
        [12u8; 8],
        asset_amount,
        collateral_atoms,
        premium_atoms,
        expiry,
        fill_deadline,
    );
    ix_fill_swap(&mut svm, &fix, &party_b, b_quote, collateral_atoms);

    let err = try_ix_cancel_swap(&mut svm, &fix, &party_a, a_asset, a_quote)
        .expect_err("cancel after fill must fail");
    assert!(
        err.contains("SwapNotCreated") || err.contains("6000") || err.contains("0x1770"),
        "unexpected error: {err}"
    );
}
