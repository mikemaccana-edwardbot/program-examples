//! `cancel_swap` — party A reclaims their locked asset and pre-funded
//! taker fee from an unfilled swap.
//!
//! Allowed states:
//! - `Created` at any time before `fill_deadline_ts` — A may cancel a
//!   change of mind or mistake before B commits.
//! - `Created` any time on/after `fill_deadline_ts` — B missed the window
//!   to fill; A reclaims everything unconditionally.
//!
//! After cancel the swap transitions to `Cancelled` (terminal) so the
//! account can no longer be filled or cancelled again.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{ASSET_VAULT_SEED, COLLATERAL_VAULT_SEED, SWAP_SEED};
use crate::errors::ErrorCode;
use crate::state::{Swap, SwapStatus};

pub fn cancel_swap(context: Context<CancelSwapAccountConstraints>) -> Result<()> {
    let swap = &context.accounts.swap;
    require!(
        swap.status == SwapStatus::Created,
        ErrorCode::SwapNotCreated
    );
    require_keys_eq!(
        swap.party_a,
        context.accounts.party_a.key(),
        ErrorCode::Unauthorized
    );

    let asset_amount = swap.amount_asset;
    let taker_fee = swap.taker_fee;

    // Build PDA signer seeds once and reuse for both transfers.
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

    // Return the locked asset to A.
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
        asset_amount,
        context.accounts.asset_mint.decimals,
    )?;

    // Return the pre-funded taker fee to A (if any).
    if taker_fee > 0 {
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
            taker_fee,
            context.accounts.quote_mint.decimals,
        )?;
    }

    let swap = &mut context.accounts.swap;
    swap.status = SwapStatus::Cancelled;
    Ok(())
}

#[derive(Accounts)]
pub struct CancelSwapAccountConstraints<'info> {
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

    #[account(address = swap.asset_mint)]
    pub asset_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(address = swap.quote_mint)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub party_a: Signer<'info>,

    pub asset_token_program: Interface<'info, TokenInterface>,
    pub quote_token_program: Interface<'info, TokenInterface>,
}
