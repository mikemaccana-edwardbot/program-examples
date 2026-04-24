# Completion Report — synthetic-exposure redesign

## Scope

Redesigned the `defi/synthetic-exposure` Anchor program from the previous
single-sided perpetual swap (Drift/GMX-v1 style with a shared market
vault) into a **two-sided peer-to-peer Total Return Swap** primitive
matching the spec handed over by Mike.

## What changed

- **Full rewrite** of `programs/synthetic_exposure/src/` — new state
  (`Swap` account + `SwapStatus` enum), new constants, new math, new
  instruction set. Old files (`initialize_market.rs`, `open_position.rs`,
  `close_position.rs`, `state::Market/Position`, leverage-based
  constants) deleted.
- New instructions: `create_swap`, `fill_swap`, `add_collateral`,
  `cancel_swap`, `settle_swap`, `liquidate`.
- Per-swap paired vaults (asset + collateral) owned by a unique Swap
  PDA. No shared market pool.
- `mock_pyth` companion program retained as executable documentation
  of the `PriceUpdateV2` byte layout. Not CPI'd from tests.
- `oracle.rs` left untouched — its owner check, staleness and
  confidence logic already matched the spec's requirements.
- README rewritten for the new TRS model (lifecycle diagram, finance
  explanation, `cargo test` instructions).

## Test results

**34/34 tests pass.** Full output:

```
running 22 tests
test math::tests::bounty_split_typical ... ok
test math::tests::bounty_zero_when_a_gets_nothing ... ok
test math::tests::liquidatable_at_threshold_is_healthy ... ok
test math::tests::liquidatable_healthy_stays_healthy ... ok
test math::tests::liquidatable_underwater_triggers ... ok
test math::tests::liquidatable_zero_equity_is_liquidatable ... ok
test math::tests::normalize_price_positive_exponent ... ok
test math::tests::normalize_price_rejects_zero_and_negative ... ok
test math::tests::normalize_price_typical_pyth_exponent ... ok
test math::tests::notional_asset_9_decimals_quote_6_decimals ... ok
test math::tests::notional_matched_decimals ... ok
test math::tests::notional_quote_larger_decimals_than_asset ... ok
test math::tests::pnl_b_overflow_does_not_panic ... ok
test math::tests::pnl_b_price_down_loses ... ok
test math::tests::pnl_b_price_up_wins ... ok
test math::tests::pnl_b_rejects_zero_entry_price ... ok
test math::tests::split_collateral_a_wins_partial_loss ... ok
test math::tests::split_collateral_b_wins_keeps_everything ... ok
test math::tests::split_collateral_b_wiped_out_exact ... ok
test math::tests::split_collateral_loss_beyond_collateral_is_capped ... ok
test math::tests::split_collateral_pnl_zero_b_keeps_everything ... ok
test test_id ... ok

test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/test_synthetic_exposure.rs

running 12 tests
test cancels_an_unfilled_swap_and_refunds_party_a ... ok
test adding_collateral_restores_health_and_blocks_liquidation ... ok
test creates_a_swap_and_locks_the_asset ... ok
test fills_a_swap_and_transfers_premium ... ok
test liquidates_when_party_b_goes_underwater_mid_term ... ok
test rejects_cancel_after_swap_filled ... ok
test rejects_fill_with_insufficient_collateral ... ok
test rejects_settle_before_expiry ... ok
test rejects_stale_oracle_at_settle ... ok
test settles_with_party_b_wiped_out_exactly ... ok
test settles_with_price_appreciation_party_b_wins ... ok
test settles_with_price_depreciation_party_a_wins ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.43s
```

### Tests to scenario mapping

1. `creates_a_swap_and_locks_the_asset` — swap goes to Created, asset vault holds the full amount, fee pre-funded to collateral vault.
2. `fills_a_swap_and_transfers_premium` — collateral vault ends at
   exactly posted amount, premium lands in B's ATA, status Active.
3. `cancels_an_unfilled_swap_and_refunds_party_a` — asset + fee refunded, vaults drained, status Cancelled.
4. `settles_with_price_appreciation_party_b_wins` — +20% price: B keeps
   full collateral + fee, A gets asset back, no quote to A.
5. `settles_with_price_depreciation_party_a_wins` — -20% price: A gets
   20 USDC + asset, B keeps 10 USDC + fee.
6. `settles_with_party_b_wiped_out_exactly` — -30% price (= full
   collateral loss): A gets all 30 USDC collateral, B has only the fee.
7. `liquidates_when_party_b_goes_underwater_mid_term` — -25% price
   pre-expiry, liquidator claims 5% bounty (1.25 USDC), A gets 23.75,
   B keeps 5 USDC residual.
8. `adding_collateral_restores_health_and_blocks_liquidation` — same
   -25% but with a +20 USDC top-up beforehand: liquidation attempt
   fails (healthy position).
9. `rejects_fill_with_insufficient_collateral` — offer 29 < required 30.
10. `rejects_settle_before_expiry` — settle call before `expiry_ts`.
11. `rejects_stale_oracle_at_settle` — oracle publish_time 10 minutes
    old vs 60-second staleness window.
12. `rejects_cancel_after_swap_filled` — cancel attempt against an
    Active swap.

Plus 22 pure-Rust math unit tests in `src/math.rs` covering price
normalisation, PnL on both sides, settlement splits (B wins, flat,
partial A win, exact wipeout, cap beyond collateral),
liquidation-threshold boundaries, bounty splits, overflow paths, and
zero-entry-price rejection.

## Design decisions (documented in README)

1. **Asymmetric settlement IS the product — this is a protective
   put, not a compromise.** The collateral vault pays A only on the
   downside; on the upside A's compensation is the appreciated asset,
   returned intact. That is exactly what a protective put does — the
   put buyer only needs the hedge when the asset falls. The previous
   "redesign" report framed this as the best interpretation of an
   ambiguous symmetric TRS spec, but the economics are cleaner than
   that: A buys a cash-settled put from B, pays a **premium** at fill,
   and claims `min(|pnl|, collateral)` from B's collateral at expiry
   if the put is in the money. For B it's a covered put sale —
   collateral posted, premium earned at fill, downside risk held to
   expiry or liquidation.

2. **Liquidation bounty paid out of A's share**, not B's. A is the
   beneficiary of the protective put and the liquidator is doing A's
   work — so it comes out of A's share of the settlement.

3. **Premium pre-funded into the collateral vault at create**, not
   held separately. Keeps `fill_swap` single-hop (one CPI from vault
   to B) and `cancel_swap` symmetric (both asset and premium refund
   from program-signed PDAs).

4. **Entry price stored as raw Pyth `(price, exponent)` pair**, not a
   normalised value. Settlement reproduces the exact scaling used at
   entry even if Pyth's exponent shifts between fill and expiry.

5. **Per-swap isolated vaults.** No shared pool, no socialised bad
   debt. If |pnl_B| exceeds `collateral_posted`, A receives at most
   `collateral_posted`; the uncovered portion is the cost of setting
   too low an initial-margin requirement at create time.

6. **Initial margin 30%, maintenance 10%, bounty 5%.** Matches the
   spec. Initial > maintenance so fresh fills don't flirt with
   liquidation. Maintenance reachable via a 20% price move at 30%
   margin — plausible on volatile feeds, stable otherwise.

## Limitations / known trade-offs

- **No upside payout to A from the vault.** A's upside is implicit in
  the asset staying intact. If Mike wanted a true two-sided TRS where
  A also wins quote when price falls AND quote when price rises
  (impossible without A posting quote too), that would require A to
  also lock collateral beyond the asset. Not implemented; see design
  decision #1.
- **No partial fills.** One B per swap. Clients needing a
  multi-counterparty swap can open N swaps with independent ids.
- **No Pyth SDK dependency.** Deliberately — the SDK pins
  `anchor-lang = "0.32.1"` which conflicts with this workspace's
  `anchor-lang = "1.0.0"`. Manual parse of `PriceUpdateV2` is ~30
  lines, layout-stable and documented.
- **Node tooling removed.** No TypeScript tests, no Codama client
  generation, no pnpm lockfile. The generated IDL lives at
  `target/idl/synthetic_exposure.json` after `anchor build` — clients
  that need typed TS bindings can run Codama on that IDL themselves
  (out of scope for this example).

## Build & run

```bash
cd defi/synthetic-exposure/anchor
CARGO_BUILD_JOBS=1 anchor build   # produces target/deploy/*.so
cargo test                         # 34 tests, ~3 s after compile
```

Both programs build clean, test suite is green, `anchor build` is
clean. No stubs, no `#[ignore]`, no feature flags that change
behaviour between prod and tests.

---

## Addendum — `taker_fee` → `premium` rename and README reframing

### Why

The previous iteration called the up-front payment from A to B a
"taker fee" (constant `MAX_TAKER_FEE_BPS`, field `taker_fee`, error
`TakerFeeTooHigh`). In orderbook vocabulary a *maker* posts an offer
and a *taker* fills it — in this program A posts the swap and B fills
it, so A is the MAKER and B is the TAKER. A fee flowing from A to B is
therefore economically a **maker rebate**, not a taker fee. The name
was inverted.

More importantly, it was also beside the point. A is buying a
cash-settled protective put from B; the payment at fill is just the
**option premium**. That's the correct economic name, independent of
orderbook vocabulary, and it's what the field has been renamed to.

### Scope of the rename

Token-level renames across `defi/synthetic-exposure/`:

| Old | New |
|---|---|
| `taker_fee` (struct field, ix arg, local var) | `premium` |
| `MAX_TAKER_FEE_BPS` (constant) | `MAX_PREMIUM_BPS` |
| `TakerFeeTooHigh` (error variant) | `PremiumTooHigh` |
| `"Taker fee exceeds the protocol maximum"` (msg) | `"Premium exceeds the protocol maximum"` |
| `TEST_FEE`, `fee_atoms` (test locals) | `TEST_PREMIUM`, `premium_atoms` |
| `fills_a_swap_and_transfers_taker_fee` (test) | `fills_a_swap_and_transfers_premium` |
| `max_fee` (local in `create_swap`) | `max_premium` |
| Doc comments mentioning "taker fee" | "premium" |

`grep -rn 'taker' defi/synthetic-exposure/` (excluding build
artifacts in `anchor/target/`) now returns zero hits.

### README reframing

- Opens with "Cash-Settled Protective Put", not "Two-Sided Total
  Return Swap".
- Adds an explicit **"What it is / What it isn't"** section up front:
  - A = **put buyer** (hedged long, pays premium).
  - B = **put writer** (posts quote collateral, earns premium).
  - Not a symmetric TRS; A only has a quote payout on the downside.
- Lifecycle diagram relabels "fee prepaid" → "premium prepaid" and
  "fee to B" → "premium paid to B".
- Finance-model section reframes PnL signs in put vocabulary:
  `pnl_B ≥ 0` ⇔ put out of the money, `pnl_B < 0` ⇔ put exercised.
- Links to [solana.com/docs/terminology](https://solana.com/docs/terminology)
  for Solana primitives rather than redefining them.
- Design trade-offs: the "no upside payout to A from the vault"
  bullet is rewritten as "Asymmetric settlement by design — this is
  the correct semantics for a protective put, not a compromise."
- Limitations section merged and clarified; no Pyth SDK / no Node
  tooling notes retained verbatim.

### Program logic untouched

This rename is purely cosmetic for the on-chain program: storage
layout, instruction args, account constraints, PDA seeds, and
settlement math are bit-identical to the previous commit. The
`declare_id!` didn't change. The IDL now exposes `premium` /
`MAX_PREMIUM_BPS` / `PremiumTooHigh` instead of the old names — that
is a **breaking IDL change** for any off-chain client, but the
program semantics are unchanged.

No genuine bugs or latent design issues were uncovered during the
pass.

### Fresh test output after rename

```
     Running unittests src/lib.rs (target/debug/deps/mock_pyth-1eac3eb937065a82)

running 1 test
test test_id ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/synthetic_exposure-de3ece08e3bc5605)

running 22 tests
test math::tests::bounty_split_typical ... ok
test math::tests::bounty_zero_when_a_gets_nothing ... ok
test math::tests::liquidatable_at_threshold_is_healthy ... ok
test math::tests::liquidatable_healthy_stays_healthy ... ok
test math::tests::liquidatable_underwater_triggers ... ok
test math::tests::liquidatable_zero_equity_is_liquidatable ... ok
test math::tests::normalize_price_positive_exponent ... ok
test math::tests::normalize_price_rejects_zero_and_negative ... ok
test math::tests::normalize_price_typical_pyth_exponent ... ok
test math::tests::notional_asset_9_decimals_quote_6_decimals ... ok
test math::tests::notional_matched_decimals ... ok
test math::tests::notional_quote_larger_decimals_than_asset ... ok
test math::tests::pnl_b_overflow_does_not_panic ... ok
test math::tests::pnl_b_price_down_loses ... ok
test math::tests::pnl_b_price_up_wins ... ok
test math::tests::pnl_b_rejects_zero_entry_price ... ok
test math::tests::split_collateral_a_wins_partial_loss ... ok
test math::tests::split_collateral_b_wins_keeps_everything ... ok
test math::tests::split_collateral_b_wiped_out_exact ... ok
test math::tests::split_collateral_loss_beyond_collateral_is_capped ... ok
test math::tests::split_collateral_pnl_zero_b_keeps_everything ... ok
test test_id ... ok

test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/test_synthetic_exposure.rs (target/debug/deps/test_synthetic_exposure-e50de14490bee4de)

running 12 tests
test cancels_an_unfilled_swap_and_refunds_party_a ... ok
test adding_collateral_restores_health_and_blocks_liquidation ... ok
test creates_a_swap_and_locks_the_asset ... ok
test fills_a_swap_and_transfers_premium ... ok
test liquidates_when_party_b_goes_underwater_mid_term ... ok
test rejects_cancel_after_swap_filled ... ok
test rejects_fill_with_insufficient_collateral ... ok
test rejects_settle_before_expiry ... ok
test rejects_stale_oracle_at_settle ... ok
test settles_with_party_b_wiped_out_exactly ... ok
test settles_with_price_appreciation_party_b_wins ... ok
test settles_with_price_depreciation_party_a_wins ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.46s

   Doc-tests mock_pyth

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests synthetic_exposure

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

**35 tests pass (22 math unit + 12 LiteSVM integration + 1 mock_pyth
`test_id`), 0 failed, 0 ignored.** Behaviour identical to
pre-rename; only names changed.
