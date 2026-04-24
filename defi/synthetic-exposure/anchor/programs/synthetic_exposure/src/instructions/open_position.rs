use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::constants::{BPS_DENOMINATOR, POSITION_SEED};
use crate::errors::ErrorCode;
use crate::oracle::read_price;
use crate::state::{Market, Position, Side};

pub fn open_position(
    context: Context<OpenPositionAccountConstraints>,
    side: Side,
    collateral: u64,
    size: u64,
) -> Result<()> {
    require!(collateral > 0, ErrorCode::ZeroCollateral);
    require!(size > 0, ErrorCode::ZeroSize);
    require!(context.accounts.market.is_active, ErrorCode::MarketInactive);

    // Leverage check: size * 10_000 <= collateral * max_leverage_bps. We do
    // the arithmetic in u128 to avoid any chance of overflow on large but
    // legal inputs (u64 * 10_000 is safely within u128).
    let size_scaled = (size as u128)
        .checked_mul(BPS_DENOMINATOR)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    let max_notional = (collateral as u128)
        .checked_mul(context.accounts.market.max_leverage_bps as u128)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    require!(size_scaled <= max_notional, ErrorCode::InvalidLeverage);

    // Pull the entry price out of the oracle. The market's stored feed id
    // is the source of truth for which feed this market tracks — the
    // `PriceFeedAccount` parameter is just "whichever account happens to be
    // posted for that feed right now".
    let feed_id = context.accounts.market.pyth_feed_id;
    let price = read_price(&context.accounts.price_update, &feed_id)?;

    // Transfer collateral from owner's token account to the market vault.
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
        collateral,
        context.accounts.quote_mint.decimals,
    )?;

    let position = &mut context.accounts.position;
    position.market = context.accounts.market.key();
    position.owner = context.accounts.owner.key();
    position.side = side;
    position.collateral = collateral;
    position.size = size;
    position.entry_price = price.price;
    position.entry_price_exponent = price.exponent;
    position.opened_at = Clock::get()?.unix_timestamp;
    position.bump = context.bumps.position;

    // Update market open-interest totals for observability.
    let market = &mut context.accounts.market;
    match side {
        Side::Long => {
            market.total_long_size = market
                .total_long_size
                .checked_add(size)
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        }
        Side::Short => {
            market.total_short_size = market
                .total_short_size
                .checked_add(size)
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        }
    }

    Ok(())
}

#[derive(Accounts)]
pub struct OpenPositionAccountConstraints<'info> {
    #[account(mut)]
    pub market: Box<Account<'info, Market>>,

    #[account(
        init,
        payer = owner,
        space = Position::DISCRIMINATOR.len() + Position::INIT_SPACE,
        seeds = [POSITION_SEED, market.key().as_ref(), owner.key().as_ref()],
        bump
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

    /// CHECK: Pyth price feed account. `oracle::read_price` enforces the
    /// Pyth Receiver owner and validates the Pyth discriminator, feed id,
    /// staleness and confidence interval before we trust any of its fields.
    pub price_update: UncheckedAccount<'info>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,

    pub system_program: Program<'info, System>,
}
