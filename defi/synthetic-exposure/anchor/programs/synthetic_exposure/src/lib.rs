//! synthetic_exposure — a peer-to-peer cash-settled protective put.
//!
//! Party A (put buyer, hedged long) locks an SPL asset they already
//! hold and pre-funds a premium. Party B (put writer, short the put)
//! posts quote-token collateral to fund A's downside claim, receives
//! the premium at fill, and keeps whatever collateral isn't paid to A
//! at settlement. The locked asset always returns to A intact.
//!
//! Not a symmetric two-sided TRS: A only receives a quote payout on the
//! downside — on the upside A's compensation is the appreciated asset.
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
        premium: u64,
        expiry_ts: i64,
        fill_deadline_ts: i64,
        pyth_feed_id: [u8; 32],
    ) -> Result<()> {
        instructions::create_swap(
            context,
            swap_id_seed,
            amount_asset,
            required_collateral,
            premium,
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
