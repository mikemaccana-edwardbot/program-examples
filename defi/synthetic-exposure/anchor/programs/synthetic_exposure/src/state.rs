use anchor_lang::prelude::*;

/// Lifecycle state of a single bilateral swap.
///
/// Transitions:
/// - `Created` → `Active` via `fill_swap`
/// - `Created` → `Cancelled` via `cancel_swap`
/// - `Active`  → `Settled` via `settle_swap` or `liquidate`
///
/// Terminal states (`Settled`, `Cancelled`) refuse every mutating
/// instruction. We keep the Swap account alive after settlement rather
/// than closing it so off-chain indexers can always read the final state.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum SwapStatus {
    Created,
    Active,
    Settled,
    Cancelled,
}

/// A single bilateral protective put between party A (put buyer — locks
/// the asset and pays the premium) and party B (put writer — posts quote
/// collateral and earns the premium).
///
/// PDA seeds: `["swap", creator, swap_id_seed]` — the 8-byte `swap_id_seed`
/// is client-supplied so one wallet can run many concurrent swaps.
///
/// Vault PDAs owned by this Swap (as token authority):
/// - Asset vault:       `["asset_vault", swap]` — holds A's locked asset.
/// - Collateral vault:  `["collateral_vault", swap]` — holds B's posted
///   quote collateral plus A's pre-funded premium (until fill releases it to B).
#[account]
#[derive(InitSpace)]
pub struct Swap {
    /// Party A — the creator, asset-locker, put buyer. Pays the premium
    /// at create time and claims the put payoff from B's collateral at
    /// settlement if the price has fallen. Rent for the Swap account is
    /// refunded to this wallet on `cancel_swap` (before fill) or left
    /// in place after settlement.
    pub party_a: Pubkey,

    /// Party B — the put writer. Posts quote-token collateral that funds
    /// A's downside claim, receives the premium at fill. `None` until
    /// `fill_swap`, after which it is set and never changed.
    pub party_b: Option<Pubkey>,

    /// SPL mint of the asset party A locks in the asset vault.
    pub asset_mint: Pubkey,

    /// SPL mint of the quote token B posts collateral in and all PnL is
    /// settled in. Typically a USDC-style 6-decimal stablecoin.
    pub quote_mint: Pubkey,

    /// Asset vault — ATA-style token account owned by the Swap PDA.
    pub asset_vault: Pubkey,

    /// Collateral vault — ATA-style token account owned by the Swap PDA.
    pub collateral_vault: Pubkey,

    /// Raw amount of asset tokens party A locked at create time. Expressed
    /// in asset-mint atoms (respects the mint's `decimals`).
    pub amount_asset: u64,

    /// Entry price `P₀` as reported by Pyth at `create_swap`, stored as the
    /// raw (price, exponent) pair so settlement can reproduce the exact
    /// normalisation scaling that was in force at entry.
    pub entry_price_raw: i64,
    pub entry_price_exponent: i32,

    /// Notional value of the locked asset at entry, in quote-mint atoms.
    /// Pre-computed at create so `settle_swap` doesn't need to recompute.
    /// Used as the PnL multiplier and as the denominator for
    /// maintenance-margin checks.
    pub notional_quote: u64,

    /// Collateral amount party B must post at `fill_swap`. Denominated in
    /// quote-mint atoms. Party A chooses this value at create time; the
    /// program enforces it is at least `INITIAL_MARGIN_BPS * notional`.
    pub required_collateral: u64,

    /// Collateral currently held in the collateral vault on B's behalf.
    /// Tracks top-ups via `add_collateral` and is the reference balance
    /// used by `liquidate` and `settle_swap`.
    pub collateral_posted: u64,

    /// Option premium party A (put buyer) pays party B (put writer) at
    /// fill, in quote-mint atoms. Economically: A is buying a cash-settled
    /// protective put on the locked asset; the premium is B's compensation
    /// for taking on the downside risk between fill and expiry. Transferred
    /// from the collateral vault to B's token account at fill — A pre-funds
    /// it into the collateral vault at create time so the program never
    /// needs to coordinate a separate transfer.
    pub premium: u64,

    /// Unix timestamp at which the swap matures and `settle_swap` becomes
    /// callable by either party.
    pub expiry_ts: i64,

    /// Deadline for party B to fill the swap. After this timestamp party A
    /// can reclaim their asset + premium via `cancel_swap`, even without B ever
    /// having cancelled explicitly. Must satisfy `< expiry_ts`.
    pub fill_deadline_ts: i64,

    /// 32-byte Pyth feed id this swap settles against. Passed by the
    /// client at create; stored so `settle_swap`/`liquidate` can refuse
    /// price updates from a different feed.
    pub pyth_feed_id: [u8; 32],

    /// 8-byte unique id supplied by the creator to distinguish multiple
    /// swaps opened from the same wallet (used as PDA seed material).
    pub swap_id_seed: [u8; 8],

    /// Current lifecycle state.
    pub status: SwapStatus,

    /// Bumps for the three PDAs so we can sign transfers from the vaults
    /// without re-deriving bumps on every instruction.
    pub bump: u8,
    pub asset_vault_bump: u8,
    pub collateral_vault_bump: u8,
}
