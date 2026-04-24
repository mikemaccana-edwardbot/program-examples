//! `settle_swap` — expires the swap and distributes both vaults.
//!
//! Callable at or after `swap.expiry_ts` by anyone (in practice it'll be
//! one of the two parties; the caller's wallet is not referenced in the
//! accounting). Settlement does NOT require the caller to be a signer on
//! A or B's token accounts — they've been fixed on the Swap state and
//! are looked up by pubkey.
//!
//! Flow:
//! 1. Read current oracle price `P₁` (same feed id as entry).
//! 2. Compute B's PnL: `notional * (P₁ - P₀) / P₀`.
//! 3. Split B's collateral between A and B per
//!    [`crate::math::split_collateral_at_settlement`].
//! 4. Return the locked asset to A in full.
//! 5. Transition state to `Settled`.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{ASSET_VAULT_SEED, COLLATERAL_VAULT_SEED, SWAP_SEED};
use crate::errors::ErrorCode;
use crate::math::{
    compute_pnl_b, normalize_price, split_collateral_at_settlement,
};
use crate::oracle::read_price;
use crate::state::{Swap, SwapStatus};

pub fn settle_swap(context: Context<SettleSwapAccountConstraints>) -> Result<()> {
    let swap = &context.accounts.swap;
    require!(swap.status == SwapStatus::Active, ErrorCode::SwapNotActive);
    let now = Clock::get()?.unix_timestamp;
    require!(now >= swap.expiry_ts, ErrorCode::NotYetExpired);

    let party_b_key = swap.party_b.ok_or_else(|| error!(ErrorCode::SwapNotActive))?;
    require_keys_eq!(
        party_b_key,
        context.accounts.party_b_quote_account.owner,
        ErrorCode::Unauthorized
    );
    require_keys_eq!(
        swap.party_a,
        context.accounts.party_a_asset_account.owner,
        ErrorCode::Unauthorized
    );
    require_keys_eq!(
        swap.party_a,
        context.accounts.party_a_quote_account.owner,
        ErrorCode::Unauthorized
    );

    let feed_id = swap.pyth_feed_id;
    let current = read_price(&context.accounts.price_update, &feed_id)?;

    let entry_normalized = normalize_price(swap.entry_price_raw, swap.entry_price_exponent)
        .ok_or_else(|| error!(ErrorCode::OracleNonPositive))?;
    let current_normalized = normalize_price(current.price, current.exponent)
        .ok_or_else(|| error!(ErrorCode::OracleNonPositive))?;

    let pnl_b = compute_pnl_b(swap.notional_quote, entry_normalized, current_normalized)?;
    let (to_a, to_b) = split_collateral_at_settlement(swap.collateral_posted, pnl_b)?;

    // Prepare Swap-PDA signer seeds; reused for every vault transfer.
    let party_a_key = swap.party_a;
    let swap_id_seed = swap.swap_id_seed;
    let swap_bump = [swap.bump];
    let signer_seeds: [&[u8]; 4] = [
        SWAP_SEED,
        party_a_key.as_ref(),
        swap_id_seed.as_ref(),
        &swap_bump,
    ];
    let signer = &[&signer_seeds[..]];

    // Return the locked asset to A (always, in full).
    let amount_asset = swap.amount_asset;
    transfer_checked(
        CpiContext::new_with_signer(
            context.accounts.asset_token_program.key(),
            TransferChecked {
                from: context.accounts.asset_vault.to_account_info(),
                mint: context.accounts.asset_mint.to_account_info(),
                to: context.accounts.party_a_asset_account.to_account_info(),
                authority: context.accounts.swap.to_account_info(),
            },
            signer,
        ),
        amount_asset,
        context.accounts.asset_mint.decimals,
    )?;

    if to_a > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                context.accounts.quote_token_program.key(),
                TransferChecked {
                    from: context.accounts.collateral_vault.to_account_info(),
                    mint: context.accounts.quote_mint.to_account_info(),
                    to: context.accounts.party_a_quote_account.to_account_info(),
                    authority: context.accounts.swap.to_account_info(),
                },
                signer,
            ),
            to_a,
            context.accounts.quote_mint.decimals,
        )?;
    }

    if to_b > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                context.accounts.quote_token_program.key(),
                TransferChecked {
                    from: context.accounts.collateral_vault.to_account_info(),
                    mint: context.accounts.quote_mint.to_account_info(),
                    to: context.accounts.party_b_quote_account.to_account_info(),
                    authority: context.accounts.swap.to_account_info(),
                },
                signer,
            ),
            to_b,
            context.accounts.quote_mint.decimals,
        )?;
    }

    let swap = &mut context.accounts.swap;
    swap.collateral_posted = 0;
    swap.status = SwapStatus::Settled;
    Ok(())
}

#[derive(Accounts)]
pub struct SettleSwapAccountConstraints<'info> {
    #[account(
        mut,
        seeds = [SWAP_SEED, swap.party_a.as_ref(), swap.swap_id_seed.as_ref()],
        bump = swap.bump,
    )]
    pub swap: Box<Account<'info, Swap>>,

    #[account(
        mut,
        seeds = [ASSET_VAULT_SEED, swap.key().as_ref()],
        bump = swap.asset_vault_bump,
        token::mint = asset_mint,
    )]
    pub asset_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [COLLATERAL_VAULT_SEED, swap.key().as_ref()],
        bump = swap.collateral_vault_bump,
        token::mint = quote_mint,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Asset goes back to A.
    #[account(
        mut,
        token::mint = asset_mint,
    )]
    pub party_a_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// A's quote account — receives the downside-protection payout (or 0).
    #[account(
        mut,
        token::mint = quote_mint,
    )]
    pub party_a_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// B's quote account — receives the remainder of the collateral vault.
    #[account(
        mut,
        token::mint = quote_mint,
    )]
    pub party_b_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = swap.asset_mint)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = swap.quote_mint)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Pyth price feed. Validated by `oracle::read_price`.
    pub price_update: UncheckedAccount<'info>,

    /// Anyone can poke expiry settlement — rewards are already fixed in
    /// the split calculation. The caller pays transaction fees only.
    pub caller: Signer<'info>,

    pub asset_token_program: Interface<'info, TokenInterface>,
    pub quote_token_program: Interface<'info, TokenInterface>,
}
