//! Finance math for the synthetic perpetual primitive.
//!
//! Everything that affects money flow lives here and is covered by the unit
//! tests at the bottom of the file. No floating point anywhere — all math is
//! integer, using `i128`/`u128` for headroom above the `u64`-sized collateral
//! and size values actually stored on-chain.

use crate::constants::{BPS_DENOMINATOR, PRICE_PRECISION};
use crate::errors::ErrorCode;
use crate::state::Side;
use anchor_lang::prelude::*;

/// `log10(PRICE_PRECISION)`. Hard-coded because `PRICE_PRECISION` is a
/// u128 literal; keeping these two in lockstep is asserted in the tests.
const PRICE_PRECISION_LOG10: i32 = 12;
const _: () = assert!(PRICE_PRECISION == 1_000_000_000_000);

/// Convert a raw Pyth `(price, exponent)` pair into this program's internal
/// fixed-point scale of [`PRICE_PRECISION`] (1e12).
///
/// For an exponent `e` (usually negative, e.g. `-8`) the natural-number
/// price is `price * 10^e`. We scale that into fixed-point by additionally
/// multiplying by `PRICE_PRECISION`, so `normalized = price * 10^(12 + e)`.
///
/// Returns `None` on overflow or on non-positive prices — the caller turns
/// that into a program error.
pub fn normalize_price(raw_price: i64, exponent: i32) -> Option<u128> {
    if raw_price <= 0 {
        return None;
    }
    // Shift magnitude: result = raw_price * 10^(PRICE_PRECISION_LOG10 + exponent).
    let shift: i32 = PRICE_PRECISION_LOG10.checked_add(exponent)?;
    let base: u128 = raw_price as u128;

    if shift >= 0 {
        let factor: u128 = 10u128.checked_pow(shift as u32)?;
        base.checked_mul(factor)
    } else {
        let divisor: u128 = 10u128.checked_pow((-shift) as u32)?;
        // Integer division truncates toward zero — acceptable here because
        // price normalization is used only for equality/ratio reasoning and
        // any loss of precision is symmetric between entry and current price.
        Some(base / divisor)
    }
}

/// Compute unrealised PnL on a position, in quote-token atoms.
///
/// Formula (all integer math, with PnL rounded toward zero by integer
/// division, which rounds AGAINST the user on gains — documented in the
/// README as the intentional direction of rounding):
///
/// - `Long:  pnl = size * (current_price - entry_price) / entry_price`
/// - `Short: pnl = size * (entry_price - current_price) / entry_price`
///
/// Prices are passed in the internal fixed-point scale produced by
/// [`normalize_price`] so their exponents have already been reconciled. The
/// returned PnL is a signed delta to apply to the position's collateral.
///
/// Overflow is treated as an error (`MathOverflow`); the function never
/// panics. A zero `entry_price_normalized` returns `ZeroSize` (it implies a
/// badly-built call because we refuse to open positions with non-positive
/// prices in the first place).
pub fn compute_pnl(
    side: Side,
    size: u64,
    entry_price_normalized: u128,
    current_price_normalized: u128,
) -> Result<i128> {
    if entry_price_normalized == 0 {
        return err!(ErrorCode::MathOverflow);
    }

    let size_signed: i128 = size as i128;
    let entry_signed: i128 = i128::try_from(entry_price_normalized)
        .map_err(|_| error!(ErrorCode::MathOverflow))?;
    let current_signed: i128 = i128::try_from(current_price_normalized)
        .map_err(|_| error!(ErrorCode::MathOverflow))?;

    let price_delta: i128 = match side {
        Side::Long => current_signed
            .checked_sub(entry_signed)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?,
        Side::Short => entry_signed
            .checked_sub(current_signed)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?,
    };

    // Multiply first (checked), then divide. Doing it this order preserves
    // precision; dividing first would zero out small-move PnL on low sizes.
    let numerator: i128 = size_signed
        .checked_mul(price_delta)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    // Integer division truncates toward zero — the documented rounding rule.
    Ok(numerator / entry_signed)
}

/// Returns `true` if the position is below maintenance margin and therefore
/// eligible for liquidation.
///
/// Healthy means `equity >= maintenance`, where:
/// - `equity = collateral + pnl` (can go negative on underwater positions)
/// - `maintenance = size * maintenance_margin_bps / 10_000`
///
/// A position with `equity <= 0` is always liquidatable, regardless of
/// maintenance margin — collateral has been fully consumed.
pub fn is_liquidatable(
    collateral: u64,
    size: u64,
    pnl: i128,
    maintenance_margin_bps: u16,
) -> Result<bool> {
    let equity: i128 = (collateral as i128)
        .checked_add(pnl)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    if equity <= 0 {
        return Ok(true);
    }

    let maintenance: i128 = (size as i128)
        .checked_mul(maintenance_margin_bps as i128)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(BPS_DENOMINATOR as i128)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    Ok(equity < maintenance)
}

/// Compute the payout to a closing trader given their collateral and PnL.
///
/// Payout is clamped at zero — the program never pays out more than the
/// vault received for this position, and never owes the trader negative
/// amounts (the debt has already been absorbed by the protocol as surplus).
pub fn compute_close_payout(collateral: u64, pnl: i128) -> Result<u64> {
    let equity: i128 = (collateral as i128)
        .checked_add(pnl)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    if equity <= 0 {
        return Ok(0);
    }

    u64::try_from(equity).map_err(|_| error!(ErrorCode::MathOverflow))
}

/// Split a liquidated position's remaining equity into (bounty, surplus).
///
/// Bounty pays the liquidator for doing the work; surplus stays in the
/// vault as protocol earnings. If equity is non-positive there is nothing
/// to split and both components are zero.
pub fn compute_liquidation_split(collateral: u64, pnl: i128, bounty_bps: u128) -> Result<(u64, u64)> {
    let remaining: i128 = (collateral as i128)
        .checked_add(pnl)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    if remaining <= 0 {
        return Ok((0, 0));
    }
    let remaining_u128: u128 = remaining as u128;

    let bounty: u128 = remaining_u128
        .checked_mul(bounty_bps)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(BPS_DENOMINATOR)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    let surplus: u128 = remaining_u128
        .checked_sub(bounty)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

    let bounty_u64: u64 = u64::try_from(bounty).map_err(|_| error!(ErrorCode::MathOverflow))?;
    let surplus_u64: u64 = u64::try_from(surplus).map_err(|_| error!(ErrorCode::MathOverflow))?;

    Ok((bounty_u64, surplus_u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scaled 1.0 at PRICE_PRECISION. Useful as a baseline in tests.
    const ONE: u128 = PRICE_PRECISION;

    #[test]
    fn normalize_price_typical_pyth_exponent() {
        // Pyth exponent of -8 is common. raw=1 means actual price = 1e-8,
        // which at PRICE_PRECISION (1e12) = 10^4.
        let normalized = normalize_price(1, -8).unwrap();
        assert_eq!(normalized, 10_000);
    }

    #[test]
    fn normalize_price_positive_exponent() {
        let normalized = normalize_price(1, 2).unwrap();
        // 1 * 10^(12 + 2) = 10^14
        assert_eq!(normalized, 100_000_000_000_000);
    }

    #[test]
    fn normalize_price_rejects_non_positive() {
        assert!(normalize_price(0, -8).is_none());
        assert!(normalize_price(-1, -8).is_none());
    }

    #[test]
    fn compute_pnl_long_profit() {
        // Size = 1_000 USDC atoms. Price doubles → 100% profit → pnl == size.
        let pnl = compute_pnl(Side::Long, 1_000, ONE, ONE * 2).unwrap();
        assert_eq!(pnl, 1_000);
    }

    #[test]
    fn compute_pnl_long_loss() {
        // Size = 1_000, price halves → 50% loss → pnl = -500.
        let pnl = compute_pnl(Side::Long, 1_000, ONE * 2, ONE).unwrap();
        assert_eq!(pnl, -500);
    }

    #[test]
    fn compute_pnl_short_profit() {
        // Short with size 1_000, price halves → 50% profit → pnl = +500.
        let pnl = compute_pnl(Side::Short, 1_000, ONE * 2, ONE).unwrap();
        assert_eq!(pnl, 500);
    }

    #[test]
    fn compute_pnl_short_loss() {
        // Short with size 1_000, price doubles → 100% loss → pnl = -1_000.
        let pnl = compute_pnl(Side::Short, 1_000, ONE, ONE * 2).unwrap();
        assert_eq!(pnl, -1_000);
    }

    #[test]
    fn compute_pnl_rounds_toward_zero_on_gain() {
        // size=3, price moves 1/3 → naive would be 1, truncated toward zero
        // from an exact 1.0; moving by exactly 1 unit produces ratio that
        // truncates to 0 for a gain smaller than one atom.
        let pnl = compute_pnl(Side::Long, 3, ONE * 3, ONE * 3 + 1).unwrap();
        // size * delta = 3 * 1 = 3; entry = 3e12; 3/3e12 = 0 (rounded).
        assert_eq!(pnl, 0);
    }

    #[test]
    fn compute_pnl_rejects_zero_entry_price() {
        let result = compute_pnl(Side::Long, 1_000, 0, ONE);
        assert!(result.is_err());
    }

    #[test]
    fn compute_pnl_overflow_does_not_panic() {
        // Maximum-size position with a price change that would overflow i128
        // when multiplied should return an error, not panic.
        let result = compute_pnl(Side::Long, u64::MAX, 1, i128::MAX as u128);
        assert!(result.is_err());
    }

    #[test]
    fn is_liquidatable_healthy_returns_false() {
        // collateral=1_000, size=10_000, pnl=0, maintenance=5% → 500 required;
        // equity=1_000 >= 500. Healthy.
        let liq = is_liquidatable(1_000, 10_000, 0, 500).unwrap();
        assert!(!liq);
    }

    #[test]
    fn is_liquidatable_underwater_returns_true() {
        // collateral=1_000, size=10_000, pnl=-900 → equity=100.
        // maintenance=5% of 10_000 = 500. 100 < 500 → liquidatable.
        let liq = is_liquidatable(1_000, 10_000, -900, 500).unwrap();
        assert!(liq);
    }

    #[test]
    fn is_liquidatable_equity_non_positive_always_liquidatable() {
        let liq = is_liquidatable(1_000, 10_000, -1_000, 500).unwrap();
        assert!(liq);
        let liq = is_liquidatable(1_000, 10_000, -2_000, 500).unwrap();
        assert!(liq);
    }

    #[test]
    fn is_liquidatable_exactly_at_threshold_stays_healthy() {
        // equity == maintenance → NOT liquidatable (strict `<`). Documented.
        let liq = is_liquidatable(500, 10_000, 0, 500).unwrap();
        assert!(!liq);
    }

    #[test]
    fn compute_close_payout_clamps_at_zero() {
        assert_eq!(compute_close_payout(100, -500).unwrap(), 0);
        assert_eq!(compute_close_payout(100, -100).unwrap(), 0);
        assert_eq!(compute_close_payout(100, 0).unwrap(), 100);
        assert_eq!(compute_close_payout(100, 50).unwrap(), 150);
    }

    #[test]
    fn compute_liquidation_split_splits_correctly() {
        // collateral=1_000, pnl=0, bounty=500 bps (5%) → bounty=50, surplus=950.
        let (bounty, surplus) = compute_liquidation_split(1_000, 0, 500).unwrap();
        assert_eq!(bounty, 50);
        assert_eq!(surplus, 950);
    }

    #[test]
    fn compute_liquidation_split_zero_when_insolvent() {
        let (bounty, surplus) = compute_liquidation_split(100, -200, 500).unwrap();
        assert_eq!(bounty, 0);
        assert_eq!(surplus, 0);
    }

    #[test]
    fn compute_liquidation_split_bounty_plus_surplus_equals_remaining() {
        // No atoms are lost or created by the split.
        let (bounty, surplus) = compute_liquidation_split(1_234_567, 10_000, 500).unwrap();
        assert_eq!(bounty as u128 + surplus as u128, 1_234_567u128 + 10_000u128);
    }
}
