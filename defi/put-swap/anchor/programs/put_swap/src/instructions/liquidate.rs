//! `liquidate` — settle a distressed swap before expiry when party B's
//! equity has dropped below the maintenance-margin floor.
//!
//! Liquidation is the same as `settle_swap` at the current oracle price
//! except for two differences:
//!   1. It is only callable when `is_liquidatable(...)` returns true,
//!      guarding against opportunistic triggers.
//!   2. The `liquidator` (any signer) receives `LIQUIDATION_BOUNTY_BPS`
//!      of party A's payout share as a bounty. The rest of A's share
//!      goes to A; B's share is unchanged. Paying the bounty out of A's
//!      side (and not B's) matches the spec: "liquidator bounty" is the
//!      cost of A outsourcing the trigger work.
//!
//! Asset always returns to A. Swap transitions to `Settled` — liquidation
//! is terminal just like a normal expiry settlement.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{
    ASSET_VAULT_SEED, COLLATERAL_VAULT_SEED, LIQUIDATION_BOUNTY_BPS, MAINTENANCE_MARGIN_BPS,
    SWAP_SEED,
};
use crate::errors::ErrorCode;
use crate::math::{
    compute_pnl_b, is_liquidatable, normalize_price, split_collateral_at_settlement,
    split_liquidation_bounty,
};
use crate::oracle::read_price;
use crate::state::{Swap, SwapStatus};

pub fn liquidate(context: Context<LiquidateAccountConstraints>) -> Result<()> {
    let swap = &context.accounts.swap;
    require!(swap.status == SwapStatus::Active, ErrorCode::SwapNotActive);

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

    let current = read_price(&context.accounts.price_update, &swap.pyth_feed_id)?;
    let entry_normalized = normalize_price(swap.entry_price_raw, swap.entry_price_exponent)
        .ok_or_else(|| error!(ErrorCode::OracleNonPositive))?;
    let current_normalized = normalize_price(current.price, current.exponent)
        .ok_or_else(|| error!(ErrorCode::OracleNonPositive))?;

    let pnl_b = compute_pnl_b(swap.notional_quote, entry_normalized, current_normalized)?;

    require!(
        is_liquidatable(
            swap.collateral_posted,
            swap.notional_quote,
            pnl_b,
            MAINTENANCE_MARGIN_BPS,
        )?,
        ErrorCode::PositionHealthy
    );

    let (to_a_raw, to_b) = split_collateral_at_settlement(swap.collateral_posted, pnl_b)?;
    let (bounty, to_a) = split_liquidation_bounty(to_a_raw, LIQUIDATION_BOUNTY_BPS)?;

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

    // Asset back to A.
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
        swap.amount_asset,
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

    if bounty > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                context.accounts.quote_token_program.key(),
                TransferChecked {
                    from: context.accounts.collateral_vault.to_account_info(),
                    mint: context.accounts.quote_mint.to_account_info(),
                    to: context.accounts.liquidator_quote_account.to_account_info(),
                    authority: context.accounts.swap.to_account_info(),
                },
                signer,
            ),
            bounty,
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
pub struct LiquidateAccountConstraints<'info> {
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

    #[account(mut, token::mint = asset_mint)]
    pub party_a_asset_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = quote_mint)]
    pub party_a_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, token::mint = quote_mint)]
    pub party_b_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::authority = liquidator,
    )]
    pub liquidator_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = swap.asset_mint)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = swap.quote_mint)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Pyth price feed. Validated by `oracle::read_price`.
    pub price_update: UncheckedAccount<'info>,

    pub liquidator: Signer<'info>,

    pub asset_token_program: Interface<'info, TokenInterface>,
    pub quote_token_program: Interface<'info, TokenInterface>,
}
