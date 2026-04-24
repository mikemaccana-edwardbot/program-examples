use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{MARKET_SEED, POSITION_SEED};
use crate::errors::ErrorCode;
use crate::math::{compute_close_payout, compute_pnl, normalize_price};
use crate::oracle::read_price;
use crate::state::{Market, Position, Side};

pub fn close_position(context: Context<ClosePositionAccountConstraints>) -> Result<()> {
    let position = &context.accounts.position;
    let market = &context.accounts.market;

    let feed_id = market.pyth_feed_id;
    let current = read_price(&context.accounts.price_update, &feed_id)?;

    let entry_normalized = normalize_price(position.entry_price, position.entry_price_exponent)
        .ok_or_else(|| error!(ErrorCode::OracleMalformed))?;
    let current_normalized = normalize_price(current.price, current.exponent)
        .ok_or_else(|| error!(ErrorCode::OracleMalformed))?;

    let pnl = compute_pnl(
        position.side,
        position.size,
        entry_normalized,
        current_normalized,
    )?;

    let payout = compute_close_payout(position.collateral, pnl)?;

    // Make sure the vault actually has enough; with well-isolated positions
    // it always should, but this catches configuration bugs.
    require!(
        context.accounts.vault.amount >= payout,
        ErrorCode::InsufficientCollateral
    );

    if payout > 0 {
        let asset_symbol = market.asset_symbol;
        let market_bump = [market.bump];
        let signer_seeds: [&[u8]; 3] = [MARKET_SEED, asset_symbol.as_ref(), &market_bump];
        let signer_seeds = &[&signer_seeds[..]];

        transfer_checked(
            CpiContext::new_with_signer(
                context.accounts.token_program.key(),
                TransferChecked {
                    from: context.accounts.vault.to_account_info(),
                    mint: context.accounts.quote_mint.to_account_info(),
                    to: context.accounts.owner_token_account.to_account_info(),
                    authority: context.accounts.market.to_account_info(),
                },
                signer_seeds,
            ),
            payout,
            context.accounts.quote_mint.decimals,
        )?;
    }

    // Update open-interest totals.
    let market_mut = &mut context.accounts.market;
    match position.side {
        Side::Long => {
            market_mut.total_long_size = market_mut
                .total_long_size
                .saturating_sub(position.size);
        }
        Side::Short => {
            market_mut.total_short_size = market_mut
                .total_short_size
                .saturating_sub(position.size);
        }
    }

    Ok(())
}

#[derive(Accounts)]
pub struct ClosePositionAccountConstraints<'info> {
    #[account(mut)]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        // Closing the position refunds its rent to the owner.
        close = owner,
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

    /// CHECK: Pyth price feed. Validated by `oracle::read_price`.
    pub price_update: UncheckedAccount<'info>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}
