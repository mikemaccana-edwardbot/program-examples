use anchor_lang::prelude::*;

use crate::constants::ASSET_SYMBOL_LEN;

/// Direction of a synthetic position.
///
/// A `Long` position profits when the oracle price rises above entry; a
/// `Short` position profits when it falls. PnL is always settled in the
/// market's quote mint — no underlying asset is ever held by the program.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum Side {
    Long,
    Short,
}

/// One synthetic perpetual market per (asset_symbol).
///
/// The market PDA is the token authority for the vault, so collateral can
/// only move out via program-signed CPIs in `close_position`, `liquidate` and
/// the collateral-return paths. Every position references exactly one market.
#[account]
#[derive(InitSpace)]
pub struct Market {
    /// Authority permitted to flip `is_active` and (future) tune parameters.
    pub authority: Pubkey,

    /// Token mint used to denominate collateral and PnL. A USDC-style
    /// 6-decimal token is the intended target but any token-interface mint
    /// works.
    pub quote_mint: Pubkey,

    /// Associated token account owned by this market's PDA that pools all
    /// trader collateral and any protocol surplus left after liquidations.
    pub vault: Pubkey,

    /// Bump for the vault token account PDA.
    pub vault_bump: u8,

    /// 32-byte Pyth feed id identifying which price feed this market tracks.
    /// Bytes are raw (no hex prefix) — clients pass them pre-decoded.
    pub pyth_feed_id: [u8; 32],

    /// Maintenance margin in bps. When equity on a position drops below
    /// `size * maintenance_margin_bps / 10_000`, the position is liquidatable.
    pub maintenance_margin_bps: u16,

    /// Maximum leverage in bps. `size * 10_000 <= collateral * max_leverage_bps`.
    /// For example 100_000 bps permits up to 10x leverage.
    pub max_leverage_bps: u32,

    /// Total notional size of all currently open long positions in this
    /// market. Denominated in quote-token atoms.
    pub total_long_size: u64,

    /// Total notional size of all currently open short positions in this
    /// market. Denominated in quote-token atoms.
    pub total_short_size: u64,

    /// Human-readable asset symbol, zero-padded to `ASSET_SYMBOL_LEN`. Used
    /// as part of the PDA seeds so each asset has a deterministic address.
    pub asset_symbol: [u8; ASSET_SYMBOL_LEN],

    /// Bump for the market PDA itself.
    pub bump: u8,

    /// When false the market refuses to open or add to positions. Existing
    /// positions may still be closed or liquidated to wind down cleanly.
    pub is_active: bool,
}

/// One open synthetic position held by a single owner in a single market.
///
/// PDA seeds: `["position", market, owner]`, so each owner can hold at most
/// one position per market. Adding to an existing position of the same side
/// is done via `add_collateral`; reversing direction requires closing first.
#[account]
#[derive(InitSpace)]
pub struct Position {
    pub market: Pubkey,

    pub owner: Pubkey,

    pub side: Side,

    /// Collateral posted by the owner, in quote-token atoms.
    pub collateral: u64,

    /// Notional exposure in quote-token atoms at entry. `size / collateral`
    /// is the effective leverage (in bps: `size * 10_000 / collateral`).
    pub size: u64,

    /// Raw Pyth price (`i64`) at the time the position was opened. Paired
    /// with `entry_price_exponent` to reconstruct the fixed-point price.
    pub entry_price: i64,

    /// Pyth exponent at the time the position was opened. Typically negative
    /// (e.g. -8 for many feeds). Stored so later PnL math uses the same
    /// scaling as entry.
    pub entry_price_exponent: i32,

    /// Unix timestamp of opening, for bookkeeping / client UI.
    pub opened_at: i64,

    pub bump: u8,
}
