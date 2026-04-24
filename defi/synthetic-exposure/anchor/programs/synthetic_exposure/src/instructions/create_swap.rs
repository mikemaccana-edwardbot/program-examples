//! `create_swap` — party A locks the asset, pre-funds the taker fee, and
//! opens a new Swap in the `Created` state.
//!
//! Reading the oracle at create time locks in `P₀`. The asset's notional
//! quote value is computed once here and stored on the Swap account so
//! later instructions don't need to re-derive it (and so no price drift
//! between create and read-only views changes the economics).
//!
//! The caller pre-funds the `taker_fee` into the collateral vault at
//! create time. That keeps the fee path single-hop at `fill_swap` — B
//! just sweeps the fee out of the collateral vault, no separate transfer
//! from A is required. If the swap is cancelled before fill, the fee
//! returns to A via `cancel_swap`.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    ASSET_VAULT_SEED, BPS_DENOMINATOR, COLLATERAL_VAULT_SEED, INITIAL_MARGIN_BPS,
    MAX_TAKER_FEE_BPS, SWAP_SEED,
};
use crate::errors::ErrorCode;
use crate::math::compute_notional_quote;
use crate::oracle::read_price;
use crate::state::{Swap, SwapStatus};

pub fn create_swap(
    context: Context<CreateSwapAccountConstraints>,
    swap_id_seed: [u8; 8],
    amount_asset: u64,
    required_collateral: u64,
    taker_fee: u64,
    expiry_ts: i64,
    fill_deadline_ts: i64,
    pyth_feed_id: [u8; 32],
) -> Result<()> {
    require!(amount_asset > 0, ErrorCode::ZeroAmount);
    require!(
        context.accounts.asset_mint.key() != context.accounts.quote_mint.key(),
        ErrorCode::SameAssetAndQuote
    );

    // Timing: fill deadline must strictly precede expiry, and expiry must
    // be in the future (use SVM clock — identical to real-runtime Clock).
    let now = Clock::get()?.unix_timestamp;
    require!(expiry_ts > now, ErrorCode::ExpiryInPast);
    require!(
        fill_deadline_ts > now && fill_deadline_ts < expiry_ts,
        ErrorCode::ExpiryInPast
    );

    // Read the oracle to lock P₀. The feed id on the PriceUpdateV2 account
    // is checked against `pyth_feed_id` inside `read_price`.
    let price = read_price(&context.accounts.price_update, &pyth_feed_id)?;

    // Compute notional in quote atoms (decimal-aware).
    let notional_quote = compute_notional_quote(
        amount_asset,
        price.price,
        price.exponent,
        context.accounts.asset_mint.decimals,
        context.accounts.quote_mint.decimals,
    )?;
    require!(notional_quote > 0, ErrorCode::ZeroAmount);

    // Required collateral must be at least the initial-margin floor.
    let min_collateral: u128 = (notional_quote as u128)
        .checked_mul(INITIAL_MARGIN_BPS)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(BPS_DENOMINATOR)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    require!(
        (required_collateral as u128) >= min_collateral,
        ErrorCode::CollateralBelowInitialMargin
    );

    // Taker fee capped as a fraction of notional. A client bug that set
    // the fee equal to notional would otherwise silently transfer A's
    // entire hedge value to B.
    let max_fee: u128 = (notional_quote as u128)
        .checked_mul(MAX_TAKER_FEE_BPS)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(BPS_DENOMINATOR)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    require!((taker_fee as u128) <= max_fee, ErrorCode::TakerFeeTooHigh);

    // Transfer asset: A → asset_vault.
    transfer_checked(
        CpiContext::new(
            context.accounts.asset_token_program.key(),
            TransferChecked {
                from: context.accounts.party_a_asset_account.to_account_info(),
                mint: context.accounts.asset_mint.to_account_info(),
                to: context.accounts.asset_vault.to_account_info(),
                authority: context.accounts.party_a.to_account_info(),
            },
        ),
        amount_asset,
        context.accounts.asset_mint.decimals,
    )?;

    // Pre-fund the taker fee into the collateral vault (if non-zero) so
    // `fill_swap` doesn't need an extra signer from A. The fee will either
    // go to B at fill or back to A at cancel.
    if taker_fee > 0 {
        transfer_checked(
            CpiContext::new(
                context.accounts.quote_token_program.key(),
                TransferChecked {
                    from: context.accounts.party_a_quote_account.to_account_info(),
                    mint: context.accounts.quote_mint.to_account_info(),
                    to: context.accounts.collateral_vault.to_account_info(),
                    authority: context.accounts.party_a.to_account_info(),
                },
            ),
            taker_fee,
            context.accounts.quote_mint.decimals,
        )?;
    }

    let swap = &mut context.accounts.swap;
    swap.party_a = context.accounts.party_a.key();
    swap.party_b = None;
    swap.asset_mint = context.accounts.asset_mint.key();
    swap.quote_mint = context.accounts.quote_mint.key();
    swap.asset_vault = context.accounts.asset_vault.key();
    swap.collateral_vault = context.accounts.collateral_vault.key();
    swap.amount_asset = amount_asset;
    swap.entry_price_raw = price.price;
    swap.entry_price_exponent = price.exponent;
    swap.notional_quote = notional_quote;
    swap.required_collateral = required_collateral;
    swap.collateral_posted = 0;
    swap.taker_fee = taker_fee;
    swap.expiry_ts = expiry_ts;
    swap.fill_deadline_ts = fill_deadline_ts;
    swap.pyth_feed_id = pyth_feed_id;
    swap.swap_id_seed = swap_id_seed;
    swap.status = SwapStatus::Created;
    swap.bump = context.bumps.swap;
    swap.asset_vault_bump = context.bumps.asset_vault;
    swap.collateral_vault_bump = context.bumps.collateral_vault;

    Ok(())
}

#[derive(Accounts)]
#[instruction(swap_id_seed: [u8; 8])]
pub struct CreateSwapAccountConstraints<'info> {
    #[account(
        init,
        payer = party_a,
        space = Swap::DISCRIMINATOR.len() + Swap::INIT_SPACE,
        seeds = [SWAP_SEED, party_a.key().as_ref(), swap_id_seed.as_ref()],
        bump
    )]
    pub swap: Box<Account<'info, Swap>>,

    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Token programs declared BEFORE the init-ed vaults so the
    /// `token::token_program = ...` constraint can reference them by name.
    /// Each vault independently picks legacy Token or Token-2022.
    pub asset_token_program: Interface<'info, TokenInterface>,
    pub quote_token_program: Interface<'info, TokenInterface>,

    #[account(
        init,
        payer = party_a,
        seeds = [ASSET_VAULT_SEED, swap.key().as_ref()],
        bump,
        token::mint = asset_mint,
        token::authority = swap,
        token::token_program = asset_token_program,
    )]
    pub asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        init,
        payer = party_a,
        seeds = [COLLATERAL_VAULT_SEED, swap.key().as_ref()],
        bump,
        token::mint = quote_mint,
        token::authority = swap,
        token::token_program = quote_token_program,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = asset_mint,
        token::authority = party_a,
    )]
    pub party_a_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::authority = party_a,
    )]
    pub party_a_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: Pyth price feed account. `oracle::read_price` enforces the
    /// Pyth Receiver owner and validates discriminator/feed id/staleness
    /// /confidence before the handler trusts any of its fields.
    pub price_update: UncheckedAccount<'info>,

    #[account(mut)]
    pub party_a: Signer<'info>,

    pub system_program: Program<'info, System>,
}
