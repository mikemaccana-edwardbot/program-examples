use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::POSITION_SEED;
use crate::errors::ErrorCode;
use crate::state::{Market, Position};

pub fn add_collateral(
    context: Context<AddCollateralAccountConstraints>,
    amount: u64,
) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroCollateral);
    require!(context.accounts.market.is_active, ErrorCode::MarketInactive);

    transfer_checked(
        CpiContext::new(
            context.accounts.token_program.key(),
            TransferChecked {
                from: context.accounts.owner_token_account.to_account_info(),
                mint: context.accounts.quote_mint.to_account_info(),
                to: context.accounts.vault.to_account_info(),
                authority: context.accounts.owner.to_account_info(),
            },
        ),
        amount,
        context.accounts.quote_mint.decimals,
    )?;

    let position = &mut context.accounts.position;
    position.collateral = position
        .collateral
        .checked_add(amount)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    Ok(())
}

#[derive(Accounts)]
pub struct AddCollateralAccountConstraints<'info> {
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        seeds = [POSITION_SEED, market.key().as_ref(), owner.key().as_ref()],
        bump = position.bump,
        has_one = market,
        has_one = owner,
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(
        mut,
        address = market.vault,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::authority = owner,
    )]
    pub owner_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = market.quote_mint)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}
