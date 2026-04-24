//! synthetic_exposure — two-sided peer-to-peer Total Return Swap (TRS).
//!
//! Party A locks an asset as collateral for a SHORT position. Party B
//! posts quote-token margin for the matching LONG position. At expiry
//! (or on liquidation) the program reads Pyth, computes B's PnL, and
//! redistributes B's collateral between the two parties. The asset
//! always returns to A intact.
//!
//! See `README.md` for the lifecycle diagram, accounts layout and the
//! exact settlement math.

use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod instructions;
pub mod math;
pub mod oracle;
pub mod state;

use instructions::*;

declare_id!("ABQ6gmEnvjn8iUKz7PBBL7Mk5paXCmxHxsCPAYZq9SUe");

#[program]
pub mod synthetic_exposure {
    use super::*;

    pub fn create_swap(
        context: Context<CreateSwapAccountConstraints>,
        swap_id_seed: [u8; 8],
        amount_asset: u64,
        required_collateral: u64,
        taker_fee: u64,
        expiry_ts: i64,
        fill_deadline_ts: i64,
        pyth_feed_id: [u8; 32],
    ) -> Result<()> {
        instructions::create_swap(
            context,
            swap_id_seed,
            amount_asset,
            required_collateral,
            taker_fee,
            expiry_ts,
            fill_deadline_ts,
            pyth_feed_id,
        )
    }

    pub fn fill_swap(
        context: Context<FillSwapAccountConstraints>,
        collateral_amount: u64,
    ) -> Result<()> {
        instructions::fill_swap(context, collateral_amount)
    }

    pub fn add_collateral(
        context: Context<AddCollateralAccountConstraints>,
        amount: u64,
    ) -> Result<()> {
        instructions::add_collateral(context, amount)
    }

    pub fn cancel_swap(context: Context<CancelSwapAccountConstraints>) -> Result<()> {
        instructions::cancel_swap(context)
    }

    pub fn settle_swap(context: Context<SettleSwapAccountConstraints>) -> Result<()> {
        instructions::settle_swap(context)
    }

    pub fn liquidate(context: Context<LiquidateAccountConstraints>) -> Result<()> {
        instructions::liquidate(context)
    }
}
