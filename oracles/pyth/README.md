# Pyth Price Feed

One instruction: `read_price`. It reads a `PriceUpdateV2` account
populated by the Pyth Solana Receiver and logs the current price,
confidence, exponent and publish time.

The point is showing the account shape — once you know what a
Pyth price feed looks like, you can use it to gate swaps, liquidate
positions, or anything else that needs an external price.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [Worked example](#5-worked-example)
6. [Safety and edge cases](#6-safety-and-edge-cases)
7. [Running the tests](#7-running-the-tests)
8. [Extending the program](#8-extending-the-program)

## 1. What does this program do?

`read_price()` deserialises a `PriceUpdateV2` account (owned by
Pyth's Solana Receiver program) and logs:
- `feed_id` — 32-byte Pyth feed identifier (which asset this is).
- `price` — the quoted price, as `i64` integer.
- `conf` — confidence interval, `u64` (same units as price).
- `exponent` — negative power of 10 to apply to `price` and `conf`
  to get the real number (e.g. exponent `-8` means divide by
  `10^8`).
- `publish_time` — Unix timestamp of the publisher push.

No writes. No CPIs. No other state.

## 2. Glossary

**Oracle (tradfi analogy)**
: In traditional finance, a pricing feed is a subscription service
(e.g. Bloomberg terminal data) that tells you what an asset is
worth right now. A blockchain "oracle" is a smart-contract
equivalent: it publishes asset prices onchain so other programs
can read them.

**Pyth**
: A decentralised oracle network. Professional publishers (trading
firms, market makers) push prices to Pyth, which aggregates them
and distributes the result to multiple chains. Aggregation happens
on Pyth's own chain (Pythnet); the result is then pushed to Solana
(and other chains) via a "receiver" program.

**Pyth Solana Receiver**
: A Solana program
(`pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT`) that receives
signed price updates from Pythnet and writes them into
`PriceUpdateV2` accounts. Clients submit an update as part of
their transaction if they want a fresh price; for persistent
published feeds, anyone can keep the account updated (a "pusher").

**`PriceUpdateV2`**
: The account type holding the latest price. Owned by the Pyth
Solana Receiver. Struct exposed via the
`pyth-solana-receiver-sdk` crate. Contains a `price_message` with
the fields listed above, plus aggregation metadata.

**Feed id**
: 32-byte identifier for a specific asset/quote pair (e.g.
SOL/USD). Lookup at
[pyth.network/price-feeds](https://pyth.network/price-feeds). The
*account* holding this feed is a separate derived address; clients
map feed id → account address via Pyth's SDK.

**Confidence interval**
: An uncertainty range around the price. Real value lives within
`price ± conf` with high probability. If `conf` is huge relative
to `price`, publishers disagree and you should probably not trade
against this feed.

**Exponent**
: How to scale `price` and `conf` to a decimal number.
`actual_price = price × 10^exponent`. Typical exponents for crypto
are `-8` or `-9`.

**Publish time**
: When the aggregated update was produced. Stale feeds (e.g. from
minutes ago during a crash) can be dangerous — check staleness
before using the price.

## 3. Accounts and PDAs

| name | kind | stores | who signs |
|---|---|---|---|
| `price_update` | `PriceUpdateV2` (owned by Pyth Solana Receiver) | aggregated Pyth update | — |

No PDAs, no signers. This is a pure read.

## 4. Instruction lifecycle walkthrough

### `read_price()`

**Who:** anyone.

**Behaviour:**
1. Anchor deserialises `price_update` using the
   `PriceUpdateV2` type from `pyth-solana-receiver-sdk`. The
   deserialisation checks the account's owner matches the Pyth
   Solana Receiver program id.
2. Log all five fields.
3. Return `Ok`.

**Token movements:** none.

**State changes:** none.

**Checks:**
- Anchor's `Account<'info, PriceUpdateV2>` rejects the call if
  the passed account isn't owned by the receiver or doesn't
  deserialise as `PriceUpdateV2`.

## 5. Worked example

```
On devnet, SOL/USD is at feed account 7UVimff...

Alice calls read_price(price_update = 7UVimff...).
Program logs:
  Price feed id: [239, 13, 139, 111, ...]
  Price: 14253000000                   // price
  Confidence: 5000000                  // conf
  Exponent: -8                         // exponent
  Publish Time: 1735999120             // unix timestamp

Real SOL/USD = 14253000000 × 10^-8 = $142.53
Confidence  = 5000000    × 10^-8 = ±$0.05
Published at 1735999120 = (date, time).
```

## 6. Safety and edge cases

- **Staleness.** Published prices live forever in the account
  until someone updates them. Nothing stops you from reading a
  two-hour-old price and thinking it's current. Every real use
  should do:
  ```
  let now = Clock::get()?.unix_timestamp;
  require!(now - publish_time < MAX_AGE_SECONDS, ErrCode::StalePrice);
  ```
- **Confidence ratio.** If `conf / price > some_threshold` (often
  1%), trade carefully — the aggregate is uncertain. Production
  code should bail when the market is this noisy.
- **Exponent sign.** Always negative in practice. Don't hardcode a
  divisor; read `exponent` and scale accordingly. A protocol that
  assumes `-8` will break silently if Pyth ever publishes a feed
  at `-6`.
- **Integer overflow.** `i64 × 10^(-exponent)` can overflow if
  you're not careful — use `i128` or fixed-point math for
  calculations.
- **Permissioned updates.** Anyone can call the receiver's
  `update_price` instruction if they have a signed Pythnet update.
  In practice, pusher programs run continuously on mainnet to keep
  the feeds fresh.

## 7. Running the tests

```bash
# Anchor
cd anchor
anchor build
anchor test

# Quasar
cd quasar
cargo test
```

The tests point at a specific `PriceUpdateV2` account (e.g. the
mainnet SOL/USD feed, cloned into the local validator via
`Anchor.toml`'s `[test.validator.clone]` or a test fixture). Make
sure the clone is present before running.

## 8. Extending the program

- **Enforce staleness.** Add `require!(Clock::get()?.unix_timestamp
  - publish_time < 60, Stale)` to reject updates older than 60
  seconds.
- **Gate on confidence.** Reject if `conf × 100 > price` (1%
  maximum uncertainty).
- **Convert and emit.** Compute the decimal price in basis-points
  and `set_return_data` it, so callers can read without
  re-deserialising.
- **Wrap in a safer accessor.** A `get_price_or_err(price_update,
  max_age, max_conf_bps)` helper returning a sanitised `u64` or
  `ErrCode`. Avoids copy-pasted checks in every instruction.
- **Swap gating.** Combine with `tokens/token-swap` — reject a
  swap if the post-trade implied price deviates from the oracle
  by more than a threshold. Teaches how DeFi apps prevent sandwich
  and depeg exploits.

## Reference

- [Pyth Solana docs](https://docs.pyth.network/solana-price-feeds)
- [Available price feeds](https://pyth.network/price-feeds?cluster=mainnet-beta)
- [`pyth-solana-receiver-sdk` crate](https://docs.rs/pyth-solana-receiver-sdk/)
