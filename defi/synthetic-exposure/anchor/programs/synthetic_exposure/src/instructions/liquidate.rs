use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{LIQUIDATION_BOUNTY_BPS, MARKET_SEED, POSITION_SEED};
use crate::errors::ErrorCode;
use crate::math::{compute_liquidation_split, compute_pnl, is_liquidatable, normalize_price};
use crate::oracle::read_price;
use crate::state::{Market, Position, Side};

pub fn liquidate(context: Context<LiquidateAccountConstraints>) -> Result<()> {
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

    require!(
        is_liquidatable(
            position.collateral,
            position.size,
            pnl,
            market.maintenance_margin_bps,
        )?,
        ErrorCode::PositionNotLiquidatable
    );

    let (bounty, _surplus) =
        compute_liquidation_split(position.collateral, pnl, LIQUIDATION_BOUNTY_BPS)?;

    // Sanity: vault must cover the bounty (surplus stays put so we don't
    // need to cover it with a transfer).
    require!(
        context.accounts.vault.amount >= bounty,
        ErrorCode::InsufficientCollateral
    );

    if bounty > 0 {
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
                    to: context.accounts.liquidator_token_account.to_account_info(),
                    authority: context.accounts.market.to_account_info(),
                },
                signer_seeds,
            ),
            bounty,
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
pub struct LiquidateAccountConstraints<'info> {
    #[account(mut)]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        // The position's rent is refunded to the position owner, not to the
        // liquidator — the liquidator is paid via the bounty. This keeps
        // liquidator incentives tied to the protocol-level bps value rather
        // than to rent economics.
        close = position_owner,
        seeds = [POSITION_SEED, market.key().as_ref(), position.owner.as_ref()],
        bump = position.bump,
        has_one = market,
    )]
    pub position: Box<Account<'info, Position>>,

    /// CHECK: matched against `position.owner` for the rent refund only.
    #[account(
        mut,
        address = position.owner,
    )]
    pub position_owner: UncheckedAccount<'info>,

    #[account(
        mut,
        address = market.vault,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::authority = liquidator,
    )]
    pub liquidator_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(address = market.quote_mint)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Pyth price feed. Validated by `oracle::read_price`.
    pub price_update: UncheckedAccount<'info>,

    #[account(mut)]
    pub liquidator: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}
