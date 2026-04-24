# synthetic-exposure

A peer-to-peer synthetic perpetual swap primitive for [Solana](https://solana.com/docs/terminology). Traders deposit a quote token (USDC-style) as collateral and open long or short exposure to any asset tracked by a [Pyth](https://pyth.network/) price feed. No underlying asset is ever held by the program — PnL is settled in the quote token against the oracle.

This is a **primitive**, not a full exchange. Funding rates, cross-margin, partial liquidation, fee distribution, and matching engines are deliberately out of scope and can be layered on top.

## Project layout

```
synthetic-exposure/
  anchor/
    programs/
      synthetic_exposure/
        src/                                            # main program
        tests/test_synthetic_exposure.rs                # LiteSVM integration suite
      mock_pyth/                                        # reference PriceUpdateV2 writer (built but unused at test time)
    Anchor.toml
    Cargo.toml
```

The Anchor project lives under `anchor/`. Commands in this README assume you've `cd`-ed into that directory.

## Architecture

### Accounts

- **`Market`** — one per asset. PDA: `["market", asset_symbol]`, where `asset_symbol` is 16 bytes, zero-padded.  
  Holds the quote mint, the vault, the Pyth feed id, leverage and maintenance-margin caps, open-interest totals, and an `is_active` flag.
- **`Position`** — one per `(market, owner)`. PDA: `["position", market, owner]`.  
  Holds side (long/short), collateral, notional size, entry price (raw Pyth `i64` + `i32` exponent), open timestamp.
- **`vault`** — PDA token account at `["vault", market]`. Owned by the market PDA; this program is the only thing that can move tokens out of it.

### Instruction flow

```
initialize_market ──▶ Market + vault ─┐
                                       ├──▶ open_position ──▶ Position (vault+= collateral)
                                       │                 │
                                       │                 ├──▶ add_collateral (vault += amount)
                                       │                 │
                                       │                 ├──▶ close_position (vault -= payout, Position closed, rent → owner)
                                       │                 │
                                       │                 └──▶ liquidate (vault -= bounty, Position closed, rent → owner, bounty → liquidator)
                                       │
                                       └── authority can mark inactive (future extension; is_active flag already in place)
```

- `open_position(side, collateral, size)` — pulls `collateral` from owner's token account into the vault, verifies `size * 10_000 ≤ collateral * max_leverage_bps`, records the current Pyth price as entry.
- `add_collateral(amount)` — tops up an existing position without changing size or entry price.
- `close_position()` — reads the current Pyth price, computes PnL, pays `max(0, collateral + pnl)` back to the owner, closes the `Position` account (rent refunded to the owner).
- `liquidate()` — anyone may call it. Computes PnL and health; if the position is underwater (equity < maintenance OR equity ≤ 0), pays 5% of remaining equity to the caller as bounty, lets the rest remain in the vault as protocol surplus, closes the `Position` (rent refunded to the position's original owner).

## Finance model

All finance math lives in [`programs/synthetic_exposure/src/math.rs`](./anchor/programs/synthetic_exposure/src/math.rs) and is unit-tested with 19 cases.

### Price normalisation

Pyth reports prices as `i64 price` + `i32 exponent`. The module converts `(price, exponent)` to an internal `u128` fixed-point scale of `PRICE_PRECISION = 1e12`:

```
normalized = price * 10^(12 + exponent)
```

This scale fits any Pyth exponent we've seen (typically -8 to -5) inside a `u128`, leaving plenty of headroom when multiplying by `u64`-sized position sizes.

### Unrealised PnL

```
Long:  pnl = size * (current_price - entry_price) / entry_price
Short: pnl = size * (entry_price - current_price) / entry_price
```

All operations use `checked_*` variants; overflow turns into a `MathOverflow` error rather than a panic. Integer division truncates toward zero, which **rounds against the user on gains**: a profitable position that earns a fractional atom rounds it to zero. This is the safer side to err on.

### Health / liquidation

A position is liquidatable when either:
- `equity ≤ 0`, or
- `equity < maintenance`, where `maintenance = size * maintenance_margin_bps / 10_000`.

`equity = collateral + pnl`. The comparison is strict: a position exactly at the threshold stays healthy.

The liquidation bounty is **5% of remaining equity** (`LIQUIDATION_BOUNTY_BPS = 500`). The remaining 95% stays in the vault as protocol surplus — this is simpler than burning, avoids the liquidator being incentivised to aggressively underwater positions, and gives the protocol operator discretion over what to do with the surplus later.

### Oracle validation

- **Staleness**: `publish_time + 60s < now` ⇒ `OracleStale` (see `STALENESS_MAX_SECONDS`).
- **Confidence**: `conf / |price| > 1%` ⇒ `OracleConfidenceTooWide` (see `MAX_CONF_BPS = 100`).
- **Positive price**: a non-positive reported price is rejected outright.
- **Feed id match**: the market's stored `pyth_feed_id` must match the bytes inside the price update account.
- **Owner**: in production builds, the account must be owned by the Pyth Receiver program at `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`.

## Pyth: direct byte parsing, no SDK dependency

This repo reads Pyth `PriceUpdateV2` bytes **directly** (see `oracle.rs`) rather than via `pyth-solana-receiver-sdk`. At the time of writing, the Pyth SDK pins `anchor-lang = 0.32.1`, which conflicts with the `anchor-lang = 1.0` used across the rest of the workspace. Parsing the 134-byte fixed layout ourselves is simpler than duplicating two versions of every borsh derive.

The production owner check requires every oracle account to be owned by the Pyth Receiver program (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`). This check applies in all builds — there is no longer a `test-oracle` feature flag that relaxes it. Under LiteSVM the integration tests seed price accounts with the correct owner via `LiteSVM::set_account`, which means production and test code paths read identical account metadata.

A companion `mock_pyth` program remains in the workspace as reference documentation of the 134-byte Pyth layout — its `write_price` instruction encodes the same bytes that `oracle::read_price` parses. It is no longer used by the tests; it simply builds as a second program so readers of the repo can see the forward/reverse byte code side by side.

## Running locally

**Prerequisites**: `anchor` 1.0, `rustc` with the Solana BPF toolchain, and `solana-cli` (for `cargo build-sbf`).

```bash
cd anchor
anchor build --ignore-keys --no-idl
cargo test
```

- `anchor build` compiles both programs to `target/deploy/*.so`. `--ignore-keys` skips the program-id-vs-keypair check (the committed source uses the authored program ids, not whatever local `anchor build` may have stashed in `target/deploy/*-keypair.json`). `--no-idl` skips the IDL generation pass; we don't need the IDL for these tests.
- `cargo test` then runs:
  - 19 `math.rs` unit tests
  - 8 LiteSVM integration tests in `programs/synthetic_exposure/tests/test_synthetic_exposure.rs`, covering market init, long/short opens and closes, add-collateral, liquidation, over-leverage rejection, and stale-oracle rejection.

The integration tests load both `.so` files into a LiteSVM instance via `include_bytes!`, seed oracle accounts directly (no live validator needed), and run end-to-end in under two seconds.

## Known limitations / deliberate scope

- **No funding rate.** A funding-rate mechanism for mean-reverting long/short imbalance is a layer above this primitive, not part of it. Each position settles purely on spot-vs-entry PnL.
- **No partial liquidation.** Liquidations close the whole position; a partial variant that keeps the position alive up to health ≥ 1 is a reasonable extension.
- **No cross-margin.** Each position is independent — the owner can't use profits in one position to support another.
- **No fee distribution / protocol surplus payout.** Surplus from liquidations simply accumulates in the vault. A future upgrade could route it to a treasury or LP pool.
- **No market-maker incentives** — this is a 1-to-1 trader-vs-vault primitive.
- **Vault capitalisation is external.** Trader profits are paid out of the vault, which must therefore be seeded with protocol-provided capital at market creation. The example tests fund each vault with 10 000 quote-token units via `mintTo` after `initialize_market`. A production deployment would typically capitalise the vault from a treasury.

## Licence

MIT.
