//! Named constants for the put-swap protective-put primitive.
//!
//! Every magic number the program and its tests touch lives here with a
//! short justification. Keep it flat — no feature flags, no config knobs —
//! so the economic parameters are trivially auditable.

/// Seed prefix for the [`crate::state::Swap`] account PDA. Combined with the
/// creator pubkey and a client-supplied 8-byte id so one wallet can open
/// many concurrent swaps without colliding.
pub const SWAP_SEED: &[u8] = b"swap";

/// Seed prefix for the asset vault PDA (party A's locked tokens). One vault
/// per swap; derived from the swap PDA.
pub const ASSET_VAULT_SEED: &[u8] = b"asset_vault";

/// Seed prefix for the collateral vault PDA (party B's posted margin in
/// quote tokens). One vault per swap.
pub const COLLATERAL_VAULT_SEED: &[u8] = b"collateral_vault";

/// Basis-point denominator. 10_000 bps == 100%.
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Initial margin requirement for party B at `fill_swap`, expressed as a
/// fraction of notional. 3_000 bps = 30% — B must post collateral worth at
/// least 30% of the asset's quote-denominated notional at entry. This
/// matches the spec: "collateral sized to cover expected downside for A
/// (e.g. 30% of notional)".
///
/// NOTE: this is a *minimum* — party A may demand more when creating the
/// swap. The on-chain check at fill uses `swap.required_collateral`, which
/// the `create_swap` handler validates `>= notional * 30%`.
pub const INITIAL_MARGIN_BPS: u128 = 3_000;

/// Maintenance-margin threshold for liquidation. 1_000 bps = 10%. If B's
/// remaining equity (`collateral + pnl_quote`) drops below
/// `notional * 10%`, anyone may call `liquidate`.
///
/// Chosen strictly lower than `INITIAL_MARGIN_BPS` so healthy positions
/// don't flirt with liquidation immediately after fill.
pub const MAINTENANCE_MARGIN_BPS: u128 = 1_000;

/// Percentage of the liquidator's recoverable collateral paid as bounty.
/// 500 bps = 5%. The rest is distributed according to the standard
/// settlement logic (A and B each take their share). The bounty comes out
/// of whatever would otherwise have gone to party A, because A is the
/// beneficiary of the protective put and the liquidator is effectively
/// performing A's work for them.
pub const LIQUIDATION_BOUNTY_BPS: u128 = 500;

/// Hard cap on the premium party A may offer at create time. 500 bps of
/// notional = 5%. Anything above this almost certainly reflects a client
/// bug (e.g. confusing a dollar amount with basis points) so we refuse.
pub const MAX_PREMIUM_BPS: u128 = 500;

/// Maximum staleness, in seconds, for a Pyth price update before the
/// program refuses to use it. 60 seconds matches Pyth's own suggested
/// default for retail integrations.
pub const STALENESS_MAX_SECONDS: u64 = 60;

/// Maximum accepted Pyth confidence interval as a fraction of the price,
/// in bps. 100 bps == 1%. Wider than this the price is too uncertain to
/// settle against.
pub const MAX_CONF_BPS: u128 = 100;

/// Internal fixed-point scale used for price math. 1e12 comfortably covers
/// every Pyth exponent we see in practice (typically -8 to -5) while still
/// leaving headroom inside u128 to multiply by u64-sized token amounts
/// (`u64::MAX` ≈ 1.8e19).
pub const PRICE_PRECISION: u128 = 1_000_000_000_000;
