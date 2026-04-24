//! Integer math for the two-sided TRS primitive.
//!
//! Zero floating point anywhere; everything is u128/i128 with overflow
//! checks. The helpers here are the single source of truth for:
//!   - translating Pyth (`price`, `exponent`) into an internal fixed-point
//!     scale suitable for multiplication against u64 token amounts,
//!   - computing a swap's notional value in quote atoms,
//!   - computing party B's PnL at settlement,
//!   - splitting the collateral vault between A and B at settlement, and
//!   - deciding whether B is liquidatable before expiry.
//!
//! Rounding policy: every integer division truncates toward zero. This is
//! the only rounding direction we use, and we explicitly choose the order
//! of multiplication vs. division in each formula so the side losing atoms
//! is always the one about to receive a payout (i.e. rounding is "against
//! the paying party" in the spec's language — residuals stay in the vault
//! and are claimable by the side taking the remainder).

use crate::constants::{BPS_DENOMINATOR, PRICE_PRECISION};
use crate::errors::ErrorCode;
use anchor_lang::prelude::*;

/// `log10(PRICE_PRECISION)`. Recomputed at compile time below to keep the
/// two constants in lockstep — if one changes the `assert!` fails fast.
const PRICE_PRECISION_LOG10: i32 = 12;
const _: () = assert!(PRICE_PRECISION == 1_000_000_000_000);

/// Convert a raw Pyth `(price, exponent)` pair into the internal fixed-point
/// scale of [`PRICE_PRECISION`] (1e12). Returns `None` if the price is
/// non-positive or on integer overflow — the caller turns that into a
/// program error.
pub fn normalize_price(raw_price: i64, exponent: i32) -> Option<u128> {
    if raw_price <= 0 {
        return None;
    }
    // normalized = raw_price * 10^(PRICE_PRECISION_LOG10 + exponent)
    let shift: i32 = PRICE_PRECISION_LOG10.checked_add(exponent)?;
    let base: u128 = raw_price as u128;
    if shift >= 0 {
        let factor: u128 = 10u128.checked_pow(shift as u32)?;
        base.checked_mul(factor)
    } else {
        // Integer-truncate on the way down; the `None` return keeps the
        // caller honest about any divide-by-zero edge case (can't happen
        // here because 10^n > 0 for n > 0).
        let divisor: u128 = 10u128.checked_pow((-shift) as u32)?;
        Some(base / divisor)
    }
}

/// Notional quote value of `amount_asset` atoms valued at raw Pyth price
/// `(entry_price, exponent)`, adjusted for the asset/quote mint decimals.
///
/// Pyth publishes a price in whole quote-unit per whole asset-unit terms
/// (e.g. 100 USDC per 1 SOL), but on-chain token amounts are in atoms. To
/// convert atoms-to-atoms cleanly we multiply by
/// `10^quote_decimals / 10^asset_decimals`:
///
/// `notional_atoms = amount_asset_atoms
///                   * normalized_price
///                   / PRICE_PRECISION
///                   * 10^quote_decimals
///                   / 10^asset_decimals`
///
/// All arithmetic stays in u128. We fold the two decimal factors into a
/// single signed `decimal_shift` so we do at most one extra multiply or
/// divide. Doing the multiplies first (when the shift is positive)
/// preserves precision for small prices.
pub fn compute_notional_quote(
    amount_asset: u64,
    entry_price: i64,
    entry_exponent: i32,
    asset_decimals: u8,
    quote_decimals: u8,
) -> Result<u64> {
    let normalized: u128 = normalize_price(entry_price, entry_exponent)
        .ok_or_else(|| error!(ErrorCode::OracleNonPositive))?;
    let product: u128 = (amount_asset as u128)
        .checked_mul(normalized)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    // Fold `/ PRICE_PRECISION * 10^quote_decimals / 10^asset_decimals`
    // into `* 10^shift` or `/ 10^(-shift)` with a single exponent.
    let shift: i32 = (quote_decimals as i32)
        .checked_sub(asset_decimals as i32)
        .and_then(|net_decimals| net_decimals.checked_sub(PRICE_PRECISION_LOG10))
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    let notional: u128 = if shift >= 0 {
        let factor: u128 = 10u128
            .checked_pow(shift as u32)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        product
            .checked_mul(factor)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?
    } else {
        let divisor: u128 = 10u128
            .checked_pow((-shift) as u32)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        product / divisor
    };
    u64::try_from(notional).map_err(|_| error!(ErrorCode::MathOverflow))
}

/// PnL to party B (the long side) at settlement, signed, in quote atoms.
///
/// Formula: `pnl_b = notional * (P₁ - P₀) / P₀`
///
/// Both prices are passed already-normalized via [`normalize_price`] so
/// their Pyth exponents have been reconciled. Positive return = price rose
/// = B wins. Negative return = price fell = A wins.
///
/// Overflow is an error — the function never panics.
pub fn compute_pnl_b(
    notional_quote: u64,
    entry_normalized: u128,
    current_normalized: u128,
) -> Result<i128> {
    if entry_normalized == 0 {
        return err!(ErrorCode::OracleNonPositive);
    }
    let notional_signed: i128 = notional_quote as i128;
    let entry_signed: i128 = i128::try_from(entry_normalized)
        .map_err(|_| error!(ErrorCode::MathOverflow))?;
    let current_signed: i128 = i128::try_from(current_normalized)
        .map_err(|_| error!(ErrorCode::MathOverflow))?;
    let delta: i128 = current_signed
        .checked_sub(entry_signed)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    let numerator: i128 = notional_signed
        .checked_mul(delta)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    // Integer division truncates toward zero — any residual atom stays in
    // the collateral vault and is swept by the party taking the remainder.
    Ok(numerator / entry_signed)
}

/// At settlement, decide how B's posted collateral is split between A and B.
///
/// Returns `(to_party_a, to_party_b)`, both in quote-mint atoms.
/// `to_party_a + to_party_b <= collateral_posted` — any unassigned atom
/// remains in the vault (truncation residual) and is intentionally stranded.
///
/// Economic semantics:
/// - `pnl_b >= 0` (price rose, B's long position wins): B keeps all
///   collateral; A receives nothing from the collateral vault because A
///   already retains the appreciated asset.
/// - `pnl_b < 0` (price fell, A's short position wins): A claims
///   `min(|pnl_b|, collateral_posted)` from the collateral vault; B keeps
///   the remainder. This is the "downside-protection" leg — A paid the
///   taker fee to insure against a price fall, B's collateral funds that
///   insurance.
pub fn split_collateral_at_settlement(
    collateral_posted: u64,
    pnl_b: i128,
) -> Result<(u64, u64)> {
    if pnl_b >= 0 {
        return Ok((0, collateral_posted));
    }
    let loss: u128 = pnl_b.unsigned_abs();
    let to_a_u128: u128 = loss.min(collateral_posted as u128);
    let to_a: u64 = u64::try_from(to_a_u128).map_err(|_| error!(ErrorCode::MathOverflow))?;
    let to_b: u64 = collateral_posted
        .checked_sub(to_a)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    Ok((to_a, to_b))
}

/// Returns `true` if party B is below maintenance margin and may be
/// liquidated. "Underwater" means `collateral_posted + pnl_b` is less than
/// `notional_quote * MAINTENANCE_MARGIN_BPS / 10_000`.
///
/// An equity of zero or below is always liquidatable: B has consumed all
/// their collateral and then some.
pub fn is_liquidatable(
    collateral_posted: u64,
    notional_quote: u64,
    pnl_b: i128,
    maintenance_margin_bps: u128,
) -> Result<bool> {
    let equity: i128 = (collateral_posted as i128)
        .checked_add(pnl_b)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    if equity <= 0 {
        return Ok(true);
    }
    let maintenance: i128 = (notional_quote as i128)
        .checked_mul(maintenance_margin_bps as i128)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(BPS_DENOMINATOR as i128)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    Ok(equity < maintenance)
}

/// Split the payout-to-A component of a liquidation into
/// `(liquidator_bounty, a_remainder)`.
///
/// The bounty is `bounty_bps` of `to_party_a` (the amount A would have
/// received in a normal settlement at current price). Paying the bounty
/// out of A's share — not B's — matches the economic intent: A is the
/// beneficiary of the protective put, and the liquidator is effectively
/// doing A's work by pulling the trigger before expiry.
///
/// Returns `(0, 0)` if there's nothing to split. Both fields fit in u64
/// because their sum equals `to_party_a` which is already a u64.
pub fn split_liquidation_bounty(to_party_a: u64, bounty_bps: u128) -> Result<(u64, u64)> {
    if to_party_a == 0 {
        return Ok((0, 0));
    }
    let bounty_u128: u128 = (to_party_a as u128)
        .checked_mul(bounty_bps)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(BPS_DENOMINATOR)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    let bounty: u64 = u64::try_from(bounty_u128).map_err(|_| error!(ErrorCode::MathOverflow))?;
    let a_remainder: u64 = to_party_a
        .checked_sub(bounty)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    Ok((bounty, a_remainder))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scaled 1.0 at PRICE_PRECISION — handy baseline for a lot of tests.
    const ONE: u128 = PRICE_PRECISION;

    // --------- normalize_price ------------------------------------------

    #[test]
    fn normalize_price_typical_pyth_exponent() {
        // Pyth exponent of -8 is common. raw=1 means actual price = 1e-8,
        // which at PRICE_PRECISION (1e12) = 10^4.
        assert_eq!(normalize_price(1, -8).unwrap(), 10_000);
    }

    #[test]
    fn normalize_price_rejects_zero_and_negative() {
        assert!(normalize_price(0, -8).is_none());
        assert!(normalize_price(-1, -8).is_none());
    }

    #[test]
    fn normalize_price_positive_exponent() {
        // 1 * 10^(12 + 2) = 10^14
        assert_eq!(normalize_price(1, 2).unwrap(), 100_000_000_000_000);
    }

    // --------- compute_notional_quote -----------------------------------

    #[test]
    fn notional_asset_9_decimals_quote_6_decimals() {
        // 1.0 SOL (9 decimals) at $100 with USDC quote (6 decimals) should
        // give 100 USDC = 100_000_000 quote atoms.
        let notional = compute_notional_quote(
            1_000_000_000,
            100 * 100_000_000, // 100 scaled at exponent -8
            -8,
            9,
            6,
        )
        .unwrap();
        assert_eq!(notional, 100_000_000);
    }

    #[test]
    fn notional_matched_decimals() {
        // 5 asset atoms at price=2, both mints 6-decimal, exponent 0.
        // Expected: 5 * 2 = 10 quote atoms.
        let notional = compute_notional_quote(5, 2, 0, 6, 6).unwrap();
        assert_eq!(notional, 10);
    }

    #[test]
    fn notional_quote_larger_decimals_than_asset() {
        // Asset 2-decimal, quote 6-decimal, price $1 (raw=1, exp=0). 100
        // asset atoms (= 1 whole unit) should give 1 whole quote = 1e6.
        let notional = compute_notional_quote(100, 1, 0, 2, 6).unwrap();
        assert_eq!(notional, 1_000_000);
    }

    // --------- compute_pnl_b --------------------------------------------

    #[test]
    fn pnl_b_price_up_wins() {
        // notional = 1_000 quote atoms. Price doubles → B PnL = +1_000.
        assert_eq!(compute_pnl_b(1_000, ONE, ONE * 2).unwrap(), 1_000);
    }

    #[test]
    fn pnl_b_price_down_loses() {
        // notional = 1_000. Price halves → B PnL = -500.
        assert_eq!(compute_pnl_b(1_000, ONE * 2, ONE).unwrap(), -500);
    }

    #[test]
    fn pnl_b_rejects_zero_entry_price() {
        assert!(compute_pnl_b(1_000, 0, ONE).is_err());
    }

    #[test]
    fn pnl_b_overflow_does_not_panic() {
        // i128 overflow on size * delta must return an error, not panic.
        assert!(compute_pnl_b(u64::MAX, 1, i128::MAX as u128).is_err());
    }

    // --------- split_collateral_at_settlement ---------------------------

    #[test]
    fn split_collateral_b_wins_keeps_everything() {
        let (to_a, to_b) = split_collateral_at_settlement(1_000, 500).unwrap();
        assert_eq!(to_a, 0);
        assert_eq!(to_b, 1_000);
    }

    #[test]
    fn split_collateral_pnl_zero_b_keeps_everything() {
        let (to_a, to_b) = split_collateral_at_settlement(1_000, 0).unwrap();
        assert_eq!(to_a, 0);
        assert_eq!(to_b, 1_000);
    }

    #[test]
    fn split_collateral_a_wins_partial_loss() {
        // A's win = 300; B has 1_000 collateral → A gets 300, B gets 700.
        let (to_a, to_b) = split_collateral_at_settlement(1_000, -300).unwrap();
        assert_eq!(to_a, 300);
        assert_eq!(to_b, 700);
    }

    #[test]
    fn split_collateral_b_wiped_out_exact() {
        // A's win exactly equals B's collateral → A gets all, B gets 0.
        let (to_a, to_b) = split_collateral_at_settlement(1_000, -1_000).unwrap();
        assert_eq!(to_a, 1_000);
        assert_eq!(to_b, 0);
    }

    #[test]
    fn split_collateral_loss_beyond_collateral_is_capped() {
        // A's "win" is bigger than B's collateral → A receives only what's
        // there; uncovered loss becomes protocol bad debt (by design, we
        // don't chase B for more).
        let (to_a, to_b) = split_collateral_at_settlement(1_000, -2_000).unwrap();
        assert_eq!(to_a, 1_000);
        assert_eq!(to_b, 0);
    }

    // --------- is_liquidatable ------------------------------------------

    #[test]
    fn liquidatable_healthy_stays_healthy() {
        // collateral=3_000, notional=10_000, pnl=0 → equity=3_000;
        // maintenance=10% of 10_000 = 1_000. 3_000 >= 1_000 → healthy.
        assert!(!is_liquidatable(3_000, 10_000, 0, 1_000).unwrap());
    }

    #[test]
    fn liquidatable_underwater_triggers() {
        // pnl=-2_500 wipes out most of 3_000 collateral → equity=500 < 1_000.
        assert!(is_liquidatable(3_000, 10_000, -2_500, 1_000).unwrap());
    }

    #[test]
    fn liquidatable_zero_equity_is_liquidatable() {
        assert!(is_liquidatable(1_000, 10_000, -1_000, 1_000).unwrap());
        assert!(is_liquidatable(1_000, 10_000, -2_000, 1_000).unwrap());
    }

    #[test]
    fn liquidatable_at_threshold_is_healthy() {
        // equity == maintenance → strictly NOT liquidatable (strict `<`).
        assert!(!is_liquidatable(1_000, 10_000, 0, 1_000).unwrap());
    }

    // --------- split_liquidation_bounty ---------------------------------

    #[test]
    fn bounty_split_typical() {
        let (bounty, a_remainder) = split_liquidation_bounty(1_000, 500).unwrap();
        assert_eq!(bounty, 50);
        assert_eq!(a_remainder, 950);
        assert_eq!(bounty + a_remainder, 1_000);
    }

    #[test]
    fn bounty_zero_when_a_gets_nothing() {
        let (bounty, a_remainder) = split_liquidation_bounty(0, 500).unwrap();
        assert_eq!(bounty, 0);
        assert_eq!(a_remainder, 0);
    }
}
