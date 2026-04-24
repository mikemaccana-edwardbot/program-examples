# Synthetic Exposure — Two-Sided Total Return Swap

A peer-to-peer primitive that lets one wallet (**party A**) lock an SPL
asset as implicit downside-protection collateral, while another wallet
(**party B**) posts stablecoin margin to take the corresponding upside
exposure. At a predetermined expiry (or earlier, via liquidation) the
program reads Pyth, computes PnL, and redistributes party B's collateral
between the two parties.

- **Party A** — asset-locker / short side. Effectively "sells" exposure
  to a named counterparty at today's oracle price, and pays a small
  taker fee for the privilege of being insured against a price fall.
- **Party B** — collateral-poster / long side. Earns the taker fee at
  fill and keeps their full collateral if price stays flat or rises.
  Loses some or all of their collateral to A if price falls.

Every swap is an isolated pair of vaults; there are no shared pools or
socialised losses. One program instance can host thousands of concurrent,
unrelated swaps.

## Finance Model

At `create_swap` the program:

1. Reads Pyth to lock the entry price `P₀`.
2. Computes the notional value in quote atoms:
   ```
   notional = amount_asset × P₀
   ```
   with a decimal adjustment so asset and quote mints can have different
   decimals. See `math::compute_notional_quote`.
3. Requires that A's chosen `required_collateral` is at least
   `INITIAL_MARGIN_BPS` of notional (default **30%**).
4. Caps the taker fee at `MAX_TAKER_FEE_BPS` of notional (default **5%**).

At `settle_swap` (or `liquidate`):

1. Read Pyth to get `P₁`.
2. Compute party B's PnL in quote atoms:
   ```
   pnl_B = notional × (P₁ − P₀) / P₀
   ```
3. Split the collateral vault:
   - **`pnl_B ≥ 0`** (price up — B wins): A gets 0 quote, B gets their
     full collateral. A keeps the appreciated asset as their "profit".
   - **`pnl_B < 0`** (price down — A wins): A claims
     `min(|pnl_B|, collateral_posted)` from the vault; B keeps the
     remainder.
4. The locked asset always returns to A in full.

Liquidation is identical except that 5% of A's payout share
(`LIQUIDATION_BOUNTY_BPS`) goes to the liquidator instead of A. The
bounty comes out of A's share — not B's — because the liquidator is
effectively performing A's work by pulling the trigger before expiry.

### Why a 30% / 10% margin schedule?

- **Initial margin (`INITIAL_MARGIN_BPS = 3_000`)**: requires B to have
  enough collateral to absorb a 30% price fall. Matches the spec's
  example and is loose enough to attract fills without being reckless.
- **Maintenance margin (`MAINTENANCE_MARGIN_BPS = 1_000`)**: liquidation
  triggers when B's equity (`collateral + pnl_B`) drops below 10% of
  notional. Strictly lower than initial margin so fresh fills never
  flirt with liquidation.

Both live in `src/constants.rs` with commentary.

### Rounding

Every integer division truncates toward zero. Whenever the truncation
direction matters (PnL, bounty split), the order of multiplication-then-
division is chosen so the side *receiving* the payout eats the rounding
residual. Stranded atoms stay in the collateral vault.

## Lifecycle

```
                  ┌───────────────┐
                  │   Created     │
                  │ (A locked,    │
                  │  fee prepaid) │
                  └──────┬────────┘
                         │
           ┌─────────────┼──────────────┐
           │             │              │
  cancel_swap(A)    fill_swap(B)        │
           │             │              │
           ▼             ▼              │
   ┌──────────────┐  ┌──────────────┐   │
   │  Cancelled   │  │    Active    │   │
   │  (terminal)  │  │ (fee to B,   │   │
   └──────────────┘  │  collateral  │   │
                     │  posted)     │   │
                     └──────┬───────┘   │
                            │           │
                 ┌──────────┼───────────┼──────────┐
                 │          │           │          │
        add_collateral  liquidate   settle_swap    │
                 │      (anyone,    (anyone, at    │
                 │       if under   or after       │
                 │       maint.)    expiry)        │
                 ▼          ▼           ▼          │
                 └───────► ┌──────────────┐        │
                           │   Settled    │        │
                           │  (terminal)  │        │
                           └──────────────┘        │
                                                   │
                (or deadline passes → A may call cancel_swap anyway)
```

## Instructions

| Instruction | Who calls | Allowed state | Effect |
|---|---|---|---|
| `create_swap` | A | *(none)* → Created | Locks `amount_asset`, pre-funds `taker_fee` into collateral vault, stores `P₀` |
| `fill_swap` | B | Created → Active | Transfers collateral to vault, releases fee to B |
| `add_collateral` | B | Active | Top-up the collateral vault, increases `collateral_posted` |
| `cancel_swap` | A | Created → Cancelled | Refunds asset and fee to A (before fill, or after deadline passes) |
| `settle_swap` | anyone | Active → Settled | At/after `expiry_ts`: reads P₁, splits vaults per settlement rules |
| `liquidate` | anyone | Active → Settled | Pre-expiry if B below maintenance margin: same as settle but pays a 5% bounty |

## Accounts & PDAs

- **Swap** — `["swap", party_a, swap_id_seed]`. One account per swap;
  the 8-byte `swap_id_seed` lets one wallet run many swaps in parallel.
- **Asset vault** — `["asset_vault", swap]`. Token account owned by the
  Swap PDA; holds A's locked asset.
- **Collateral vault** — `["collateral_vault", swap]`. Token account
  owned by the Swap PDA; holds the pre-funded fee (until fill) and B's
  collateral.

Both vaults use the Anchor `token_interface` wrappers so each swap can
independently choose legacy SPL Token or Token-2022 for asset and/or
quote.

## Oracle Integration

Prices are read from **Pyth Solana Receiver** `PriceUpdateV2` accounts.
The `oracle.rs` module deserialises the byte layout manually —
`pyth-solana-receiver-sdk` pins `anchor-lang = "0.32.1"` which conflicts
with this workspace's `anchor-lang = "1.0.0"`, so depending on the SDK
directly would force a downgrade. The layout is stable and only 134 bytes;
the trade-off is worth it.

The oracle reader enforces:
- Owner = Pyth Receiver program id (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`) — no feature-flag relaxation for tests.
- Discriminator matches `PriceUpdateV2`.
- Feed id matches the value stored on the Swap at `create_swap`.
- `publish_time + STALENESS_MAX_SECONDS (60s) ≥ now`.
- Price is strictly positive.
- `confidence ≤ MAX_CONF_BPS (100 bps) × price`.

## Test Oracle

Tests seed a Pyth-owned account directly into LiteSVM via
`LiteSVM::set_account`, with bytes that match the production
`PriceUpdateV2` layout. There is no `test-oracle` feature or alternate
code path — the same parser runs in both prod and tests.

The companion `mock_pyth` program is retained as executable
documentation of the byte layout; it's loaded into the LiteSVM suite
but not CPI'd into.

## Running the Tests

Everything is Rust. No Node toolchain, no TypeScript, no Codama, no
pnpm, no live validator. Tests are LiteSVM integration tests that
`include_bytes!` the freshly-built program `.so` files.

```
cd anchor
anchor build    # produces target/deploy/*.so
cargo test      # 22 math unit tests + 12 LiteSVM integration tests
```

Anchor.toml's `[scripts]` maps `anchor test` to `cargo test`, so
`anchor test` runs `anchor build` followed by the Rust test suite.

On machines with limited RAM set `CARGO_BUILD_JOBS=1` during the first
compile to avoid linker OOMs.

## Design Trade-offs

- **No upside payout to A from the collateral vault** — A's upside is
  implicit in the appreciated asset, which returns to A intact. This is
  the clean, balanced interpretation of the spec's "A gets asset back
  + (B's collateral − pnl); B gets their collateral + pnl" (the literal
  reading cannot balance because it requires paying out
  `2 × B's collateral` from a vault that only contains
  `B's collateral`). Party A effectively holds a protective put struck
  at `P₀`; B sells that put and earns the taker fee as premium.
- **Per-swap vaults, not pooled** — no socialised bad debt across
  swaps. If P₁ crashes far below P₀ and `|pnl_B|` exceeds
  `collateral_posted`, A receives at most `collateral_posted`; the
  uncovered portion is the cost of an insufficient initial margin.
- **Liquidation bounty out of A's share** — A is the beneficiary of
  the protective put and the liquidator is doing A's work, so it's
  A's to pay.
- **Entry price stored as raw Pyth `(price, exponent)` pair** — lets
  settlement reproduce the exact normalisation used at create time
  even if the Pyth exponent shifts between fill and expiry.

## File Layout

```
anchor/
├── Anchor.toml            # cargo test as the `test` script
├── Cargo.toml             # workspace members
└── programs/
    ├── synthetic_exposure/
    │   ├── src/
    │   │   ├── lib.rs
    │   │   ├── constants.rs
    │   │   ├── errors.rs
    │   │   ├── math.rs           # + unit tests
    │   │   ├── oracle.rs         # Pyth PriceUpdateV2 parser
    │   │   ├── state.rs          # Swap account + SwapStatus enum
    │   │   └── instructions/
    │   │       ├── mod.rs
    │   │       ├── create_swap.rs
    │   │       ├── fill_swap.rs
    │   │       ├── add_collateral.rs
    │   │       ├── cancel_swap.rs
    │   │       ├── settle_swap.rs
    │   │       └── liquidate.rs
    │   └── tests/
    │       └── test_synthetic_exposure.rs
    └── mock_pyth/
        └── src/lib.rs     # test-only PriceUpdateV2 writer
```
