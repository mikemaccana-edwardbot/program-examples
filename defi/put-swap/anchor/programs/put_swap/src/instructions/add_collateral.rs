//! `add_collateral` — party B tops up the collateral vault mid-term to
//! avoid liquidation.
//!
//! Only callable while the swap is `Active` and only by the party B who
//! filled it. The top-up increases `collateral_posted` atom-for-atom.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{COLLATERAL_VAULT_SEED, SWAP_SEED};
use crate::errors::ErrorCode;
use crate::state::{Swap, SwapStatus};

pub fn add_collateral(
    context: Context<AddCollateralAccountConstraints>,
    amount: u64,
) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);
    let swap = &context.accounts.swap;
    require!(swap.status == SwapStatus::Active, ErrorCode::SwapNotActive);
    let filled_by = swap.party_b.ok_or_else(|| error!(ErrorCode::SwapNotActive))?;
    require_keys_eq!(
        filled_by,
        context.accounts.party_b.key(),
        ErrorCode::Unauthorized
    );

    transfer_checked(
        CpiContext::new(
            context.accounts.token_program.key(),
            TransferChecked {
                from: context.accounts.party_b_quote_account.to_account_info(),
                mint: context.accounts.quote_mint.to_account_info(),
                to: context.accounts.collateral_vault.to_account_info(),
                authority: context.accounts.party_b.to_account_info(),
            },
        ),
        amount,
        context.accounts.quote_mint.decimals,
    )?;

    let swap = &mut context.accounts.swap;
    swap.collateral_posted = swap
        .collateral_posted
        .checked_add(amount)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    Ok(())
}

#[derive(Accounts)]
pub struct AddCollateralAccountConstraints<'info> {
    #[account(
        mut,
        seeds = [SWAP_SEED, swap.party_a.as_ref(), swap.swap_id_seed.as_ref()],
        bump = swap.bump,
    )]
    pub swap: Box<Account<'info, Swap>>,

    #[account(
        mut,
        seeds = [COLLATERAL_VAULT_SEED, swap.key().as_ref()],
        bump = swap.collateral_vault_bump,
        token::mint = quote_mint,
    )]
    pub collateral_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::authority = party_b,
    )]
    pub party_b_quote_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = swap.quote_mint)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub party_b: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}
