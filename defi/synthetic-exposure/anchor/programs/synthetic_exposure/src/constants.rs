//! Named constants. Every numeric value lives here so the rest of the program
//! can be read without magic numbers.

/// Seed prefix for the per-asset [`crate::state::Market`] PDA.
pub const MARKET_SEED: &[u8] = b"market";

/// Seed prefix for the per-(market, owner) [`crate::state::Position`] PDA.
pub const POSITION_SEED: &[u8] = b"position";

/// Seed prefix for the vault token account owned by the market PDA. The vault
/// holds pooled trader collateral in the quote mint (USDC-style 6-decimal
/// token).
pub const VAULT_SEED: &[u8] = b"vault";

/// Fixed byte length of a market's asset-symbol seed input. Using a fixed
/// length keeps PDA derivation deterministic across clients and avoids
/// ambiguity from trimmed strings. Zero-padded to the right.
pub const ASSET_SYMBOL_LEN: usize = 16;

/// Basis-point denominator. 10_000 bps == 100%. Used for every bps math step.
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Maximum acceptable age (seconds) for a Pyth price update. Chosen to match
/// common Pyth integrations — if the oracle has not posted in 60 seconds we
/// treat it as stale and refuse to open, close or liquidate positions.
pub const STALENESS_MAX_SECONDS: u64 = 60;

/// Maximum acceptable confidence interval as a fraction of the price, in bps.
/// 100 bps == 1%. If `conf / price` exceeds this the oracle is considered too
/// uncertain for trading, protecting the protocol from wide-spread updates
/// that could make liquidation economics unfair.
pub const MAX_CONF_BPS: u128 = 100;

/// Internal fixed-point scale used for price math. 1e12 is chosen because it
/// exceeds the natural-scale range of every Pyth exponent we've seen
/// (typically -8 to -5) while still leaving plenty of headroom inside u128 to
/// multiply by USDC-atom-sized notional values (`u64::MAX` ≈ 1.8e19). Not
/// exported through the IDL because u128 has no first-class Anchor type —
/// clients that need this value should pull it from the Rust source.
pub const PRICE_PRECISION: u128 = 1_000_000_000_000;

/// Percentage of a liquidated position's remaining collateral paid to the
/// caller of `liquidate`. 500 bps = 5%. The rest stays in the vault as
/// protocol surplus — documented in the README.
pub const LIQUIDATION_BOUNTY_BPS: u128 = 500;

/// Hard cap on `max_leverage_bps` to prevent pathological markets from being
/// created. 1_000_000 bps = 100x leverage. Markets above this refuse to init.
pub const MAX_ALLOWED_LEVERAGE_BPS: u32 = 1_000_000;

/// Hard ceiling for `maintenance_margin_bps`. 5_000 bps = 50% — above this
/// positions would be liquidatable the moment they open under any realistic
/// leverage, which is almost certainly a misconfiguration.
pub const MAX_MAINTENANCE_MARGIN_BPS: u16 = 5_000;
