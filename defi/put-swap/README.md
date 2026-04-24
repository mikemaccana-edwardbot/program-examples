# Put Swap

A peer-to-peer, on-chain, cash-settled **protective put** between two
wallets. One wallet (**party A**) hedges the downside of an SPL asset
they already hold; the other (**party B**) writes that hedge in
exchange for a premium, earning yield on idle stablecoin collateral.
At expiry (or earlier, via liquidation) the program reads
[Pyth](https://pyth.network), computes how far the price has fallen,
and pays A out of B's collateral. Per-swap isolated vaults, fixed
expiry, oracle-settled.

### Also known as / use cases

Different audiences reach for different names for this shape of
contract. They all describe the same primitive implemented here:

- **Downside hedge** — A's primary motivation: protect the value of an
  existing SPL holding against a drop over a fixed window.
- **Downside insurance** — colloquial framing used in DeFi and TradFi
  for the same hedge. Note: this program is **not** a regulated
  insurance product; it's a bilateral derivatives contract. The word
  is used here purely as a use-case synonym for "downside hedge".
- **Protective put** — the precise TradFi name for the strategy A is
  running: long the underlying + long a cash-settled put on it.
- **Cash-secured put (writer side)** — what B is doing: collecting a
  premium up front for the obligation to pay out if the underlying
  drops, backed by posted quote collateral.
- **Put swap** — the primitive name used by this example, in the same
  vein as *perpetual swap*. It's a bilateral swap contract whose
  payoff profile is that of a put.

### What it is

A cash-settled **protective put** on a Pyth-priced asset, executed as a
bilateral put swap:

- **Party A** — **put buyer** (hedged long). Already owns the asset,
  wants insurance against a price fall. Locks the asset to prove they
  hold it and pre-funds a **premium** which goes to B at fill.
- **Party B** — **put writer** (short the put). Posts quote-token
  (e.g. USDC) collateral that funds A's downside claim, earns the
  premium at fill, keeps whatever collateral isn't paid out to A at
  settlement.

At settlement, if the price has fallen, A claims
`min(|pnl|, collateral_posted)` in quote tokens from B's collateral
vault. If the price is flat or up, A gets nothing from the collateral
vault — A's upside is implicit in the appreciated asset, which is
always returned to A intact.

Every swap is an isolated pair of vaults; there are no shared pools or
socialised losses. One program instance can host thousands of concurrent,
unrelated swaps.

### What it isn't

**Not** a symmetric two-sided Total Return Swap. A only receives a
quote payout on the *downside*. On the upside A's compensation is the
asset itself, which has appreciated. That's structurally what a
protective put is — put buyers don't get more cash when their hedged
asset rallies, they just don't need the hedge. Don't use this as a
template for a truly synthetic long/short pair where both sides settle
in quote; that would require A to also post quote collateral.

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
4. Caps the premium at `MAX_PREMIUM_BPS` of notional (default **5%**).
5. Pre-funds the premium into the collateral vault. It stays there
   until fill (when it goes to B) or cancel (when it returns to A).

At `settle_swap` (or `liquidate`):

1. Read Pyth to get `P₁`.
2. Compute the put's payoff, expressed as B's PnL in quote atoms:
   ```
   pnl_B = notional × (P₁ − P₀) / P₀
   ```
   `pnl_B ≥ 0` ⇔ put is out of the money (price flat or up).
   `pnl_B < 0` ⇔ put is in the money (price down); the put writer B
   owes the put buyer A.
3. Split the collateral vault:
   - **`pnl_B ≥ 0`** (put expires worthless — B keeps the premium):
     A gets 0 quote; B gets their full collateral back. A keeps the
     appreciated asset.
   - **`pnl_B < 0`** (put is exercised against B): A claims
     `min(|pnl_B|, collateral_posted)` from the vault; B keeps the
     remainder.
4. The locked asset always returns to A in full.

Liquidation is identical except that 5% of A's payout share
(`LIQUIDATION_BOUNTY_BPS`) goes to the liquidator instead of A. The
bounty comes out of A's share — not B's — because the liquidator is
effectively performing A's work by pulling the trigger before expiry.

### Why a 30% / 10% margin schedule?

- **Initial margin (`INITIAL_MARGIN_BPS = 3_000`)**: requires B to post
  enough collateral to cover a 30% fall in the asset price. Loose
  enough to attract writers without being reckless.
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
                  ┌────────────────┐
                  │    Created     │
                  │ (A locked,     │
                  │  premium       │
                  │  prepaid)      │
                  └──────┬─────────┘
                         │
           ┌─────────────┼──────────────┐
           │             │              │
  cancel_swap(A)    fill_swap(B)        │
           │             │              │
           ▼             ▼              │
   ┌──────────────┐  ┌──────────────────┐
   │  Cancelled   │  │      Active      │
   │  (terminal)  │  │ (premium paid to │
   └──────────────┘  │  B, collateral   │
                     │  posted)         │
                     └──────┬───────────┘
                            │
                 ┌──────────┼───────────┬──────────┐
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

## Why Participate? What Each Side Gets

### Party A — Put Buyer (holds the asset)

**What A gets:**
- **Downside protection.** If the asset price falls between create and
  expiry, A is paid out of B's collateral for the drop (up to the amount
  of collateral posted). A's total return ≈ (asset value at P₁) + (B's
  collateral contribution) ≈ A's portfolio value at P₀ on the downside.
- **Asset appreciation, unaltered.** If price rises, A still holds the
  asset — they get the full upside. The program doesn't touch A's asset.
- **Capital efficiency.** A doesn't sell their position, doesn't incur
  tax events, doesn't lose staking/yield on the underlying (assuming
  they'd be earning it elsewhere — the locked asset in our vault doesn't
  earn, which is one cost).

**What A pays:**
- **Premium** (upfront, to B). The cost of the protection.
- **Opportunity cost** of the locked asset — it sits idle in the vault
  earning nothing until expiry.
- **Expiry risk** — if A needs to exit early for non-liquidation reasons,
  they can't (no early-close instruction).

**A's position is equivalent to**: buying a cash-settled put option on
their holding, with the premium paid upfront, strike = P₀, cash
settlement at expiry.

**Real-world reasons to do this**: A holds an asset long-term (taxable
lot, staking rewards, narrative conviction) but wants to hedge a specific
window of downside risk (earnings event, macro event, cliff vest,
roadmap milestone).

### Party B — Put Writer (posts collateral)

**What B gets:**
- **Premium, earned upfront** (at fill time). Pure income if the asset
  stays at or above P₀ through expiry.
- **No asset exposure required.** B doesn't need to own the asset — just
  stablecoin to post as collateral.
- **Defined max loss.** B can lose at most their posted collateral. The
  30% initial margin means B is writing a put with a 30% max loss
  ceiling. No unlimited downside (unlike an uncovered short).

**What B pays / risks:**
- **Collateral is locked** until expiry (no yield while locked).
- **Downside losses** if price falls. Losses come out of collateral
  pro-rata to the percentage drop. If asset drops 30%+, B loses their
  full stake.
- **Liquidation risk.** If price falls past maintenance margin mid-term,
  anyone can call `liquidate` and B gets settled early at the worse
  price (no chance to recover if price bounces back).
- **Opportunity cost** on the stablecoin collateral.

**B's position is equivalent to**: writing a cash-secured put — classic
income strategy in options markets. Get paid the premium, obligated to
"buy at P₀" (effectively) if price falls.

**Real-world reasons to do this**:
- B is bullish on the asset but doesn't want to buy it at P₀. They're
  willing to buy it *if* it drops. Premium compensates them for that
  commitment.
- B has idle stablecoins and wants yield from a defined-risk strategy.
- B wants to express "I don't think this asset will crash below 30% of
  P₀ in the next X days" as a trade.

### Why would A and B match?

They have **opposite views on near-term downside**:

- A is worried about downside → willing to pay to offload it.
- B thinks downside is unlikely → willing to take the risk for premium.

In efficient markets the premium will price that disagreement — volatile
assets or uncertain windows → higher premium, stable assets → lower
premium.

### What A does NOT get (worth being explicit)

- **No payout on asset appreciation from the program.** If A also wanted
  to be fully hedged (locked in P₀ regardless of direction), this isn't
  the instrument — that's a total return swap where A would also give
  up upside. This program is a put, not a TRS.
- **No early exit.** No close-before-expiry instruction. If A changes
  their mind mid-swap, they're stuck until `settle_swap` becomes
  callable at `expiry_ts`, unless B gets liquidated first.

### What B does NOT get

- **No upside from the asset.** Even if price 10x's, B still only gets
  the premium. B is capped at the premium earned.
- **No continuous fee / funding.** Unlike a perpetual, there's no
  recurring payment between sides — premium is one-shot at fill.

## Instructions

| Instruction | Who calls | Allowed state | Effect |
|---|---|---|---|
| `create_swap` | A | *(none)* → Created | Locks `amount_asset`, pre-funds `premium` into collateral vault, stores `P₀` |
| `fill_swap` | B | Created → Active | Transfers collateral to vault, releases premium to B |
| `add_collateral` | B | Active | Top-up the collateral vault, increases `collateral_posted` |
| `cancel_swap` | A | Created → Cancelled | Refunds asset and premium to A (before fill, or after deadline passes) |
| `settle_swap` | anyone | Active → Settled | At/after `expiry_ts`: reads P₁, splits vaults per settlement rules |
| `liquidate` | anyone | Active → Settled | Pre-expiry if B below maintenance margin: same as settle but pays a 5% bounty |

## Accounts & PDAs

- **Swap** — `["swap", party_a, swap_id_seed]`. One account per swap;
  the 8-byte `swap_id_seed` lets one wallet run many swaps in parallel.
- **Asset vault** — `["asset_vault", swap]`. Token account owned by the
  Swap PDA; holds A's locked asset.
- **Collateral vault** — `["collateral_vault", swap]`. Token account
  owned by the Swap PDA; holds the pre-funded premium (until fill) and
  B's collateral.

Both vaults use the Anchor `token_interface` wrappers so each swap can
independently use the legacy SPL Token program or Token-2022 for the
asset and/or quote mint. See the
[Solana terminology docs](https://solana.com/docs/terminology) for
the vocabulary this builds on (PDAs, token accounts, mints).

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

`Anchor.toml`'s `[scripts]` maps `anchor test` to `cargo test`, so
`anchor test` runs `anchor build` followed by the Rust test suite.

On machines with limited RAM set `CARGO_BUILD_JOBS=1` during the first
compile to avoid linker OOMs.

## Design Trade-offs

- **Asymmetric settlement by design.** The collateral vault only pays
  A on the downside — A's upside is the asset itself, which is always
  returned intact. This is the correct semantics for a protective put,
  not a compromise. A literal symmetric TRS (A also wins quote when
  price rises) would need A to post quote collateral too; that's a
  different product.
- **Per-swap vaults, not pooled** — no socialised bad debt across
  swaps. If `P₁` crashes far below `P₀` and `|pnl_B|` exceeds
  `collateral_posted`, A receives at most `collateral_posted`; the
  uncovered portion is the cost of an insufficient initial margin.
- **Liquidation bounty out of A's share** — A is the beneficiary of
  the protective put and the liquidator is doing A's work, so it's
  A's to pay.
- **Entry price stored as raw Pyth `(price, exponent)` pair** — lets
  settlement reproduce the exact normalisation used at create time
  even if the Pyth exponent shifts between fill and expiry.

## Limitations

- **No symmetric upside payout to A.** See "What it isn't" above.
  Party A's upside is the asset's appreciation, paid implicitly when
  the locked asset returns at settlement.
- **No partial fills.** One B per swap. Clients needing a
  multi-counterparty hedge can open N swaps with independent ids.
- **No Pyth SDK dependency.** Deliberately — the SDK pins
  `anchor-lang = "0.32.1"` which conflicts with this workspace's
  `anchor-lang = "1.0.0"`. Manual parse of `PriceUpdateV2` is ~30
  lines, layout-stable and documented.
- **No Node tooling.** No TypeScript tests, no Codama client
  generation, no pnpm lockfile. The generated IDL lives at
  `target/idl/put_swap.json` after `anchor build` — clients
  that need typed TS bindings can run Codama on that IDL themselves
  (out of scope for this example).

## File Layout

```
anchor/
├── Anchor.toml            # cargo test as the `test` script
├── Cargo.toml             # workspace members
└── programs/
    ├── put_swap/
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
    │       └── test_put_swap.rs
    └── mock_pyth/
        └── src/lib.rs     # test-only PriceUpdateV2 writer
```
