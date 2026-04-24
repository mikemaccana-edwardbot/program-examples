//! `fill_swap` — party B posts the required collateral, receives the
//! premium pre-funded by A, and activates the swap.
//!
//! Only callable while the swap is in `Created` state and the fill
//! deadline hasn't passed. The collateral transferred must be at least
//! `swap.required_collateral`; the program accepts larger amounts
//! silently (B is free to over-collateralize).

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{COLLATERAL_VAULT_SEED, SWAP_SEED};
use crate::errors::ErrorCode;
use crate::state::{Swap, SwapStatus};

pub fn fill_swap(
    context: Context<FillSwapAccountConstraints>,
    collateral_amount: u64,
) -> Result<()> {
    let swap = &context.accounts.swap;
    require!(
        swap.status == SwapStatus::Created,
        ErrorCode::SwapNotCreated
    );
    let now = Clock::get()?.unix_timestamp;
    require!(now < swap.fill_deadline_ts, ErrorCode::ExpiryInPast);
    require!(
        collateral_amount >= swap.required_collateral,
        ErrorCode::InsufficientCollateral
    );

    // Transfer collateral: B → collateral_vault.
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
        collateral_amount,
        context.accounts.quote_mint.decimals,
    )?;

    // Release pre-funded premium from the collateral vault to B. The
    // Swap PDA is the vault's token authority, so we sign with its seeds.
    let premium = swap.premium;
    if premium > 0 {
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
        transfer_checked(
            CpiContext::new_with_signer(
                context.accounts.token_program.key(),
                TransferChecked {
                    from: context.accounts.collateral_vault.to_account_info(),
                    mint: context.accounts.quote_mint.to_account_info(),
                    to: context.accounts.party_b_quote_account.to_account_info(),
                    authority: context.accounts.swap.to_account_info(),
                },
                signer,
            ),
            premium,
            context.accounts.quote_mint.decimals,
        )?;
    }

    let swap = &mut context.accounts.swap;
    swap.party_b = Some(context.accounts.party_b.key());
    swap.collateral_posted = collateral_amount;
    swap.status = SwapStatus::Active;
    Ok(())
}

#[derive(Accounts)]
pub struct FillSwapAccountConstraints<'info> {
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
