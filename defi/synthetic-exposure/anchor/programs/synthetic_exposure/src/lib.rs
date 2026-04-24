//! synthetic_exposure — peer-to-peer synthetic perpetual swap.
//!
//! Users deposit quote-token collateral (USDC-style) and open long/short
//! synthetic exposure to an asset tracked by a Pyth price feed. The program
//! never holds the underlying asset; all PnL is settled in the quote token
//! against the oracle.
//!
//! See `README.md` for the accounts, flows, and finance model.

use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod instructions;
pub mod math;
pub mod oracle;
pub mod state;

use instructions::*;
use state::Side;

declare_id!("5dwJTVySHJFvvfzstKP6eoJT3P5MXjHcMmHKfdWkUhRH");

#[program]
pub mod synthetic_exposure {
    use super::*;

    pub fn initialize_market(
        context: Context<InitializeMarketAccountConstraints>,
        asset_symbol: [u8; constants::ASSET_SYMBOL_LEN],
        pyth_feed_id: [u8; 32],
        maintenance_margin_bps: u16,
        max_leverage_bps: u32,
    ) -> Result<()> {
        instructions::initialize_market(
            context,
            asset_symbol,
            pyth_feed_id,
            maintenance_margin_bps,
            max_leverage_bps,
        )
    }

    pub fn open_position(
        context: Context<OpenPositionAccountConstraints>,
        side: Side,
        collateral: u64,
        size: u64,
    ) -> Result<()> {
        instructions::open_position(context, side, collateral, size)
    }

    pub fn close_position(context: Context<ClosePositionAccountConstraints>) -> Result<()> {
        instructions::close_position(context)
    }

    pub fn add_collateral(
        context: Context<AddCollateralAccountConstraints>,
        amount: u64,
    ) -> Result<()> {
        instructions::add_collateral(context, amount)
    }

    pub fn liquidate(context: Context<LiquidateAccountConstraints>) -> Result<()> {
        instructions::liquidate(context)
    }
}
