# Token Swap (Constant-Product AMM)

A Uniswap-V2-style Automated Market Maker in a single Anchor
program. Supports creating a fee-collecting AMM config, opening a
pool for any token-A / token-B pair, depositing and withdrawing
liquidity in exchange for LP tokens, and swapping tokens with the
constant-product invariant `x * y = k` (minus trading fees).

Enough to teach the mechanics. Not production-ready — there are a
couple of known footguns called out in the Safety section.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [The constant-product invariant](#5-the-constant-product-invariant)
6. [Worked example](#6-worked-example)
7. [Safety and edge cases](#7-safety-and-edge-cases)
8. [Running the tests](#8-running-the-tests)
9. [Extending the program](#9-extending-the-program)

## 1. What does this program do?

Five instructions:

| instruction | who calls | effect |
|---|---|---|
| `create_amm(id, fee)` | anyone | Creates an `Amm` PDA seeded by `id`, stores `admin`, `fee` (bps). One `Amm` can host many pools. |
| `create_pool()` | anyone | Creates a `Pool` PDA seeded by `(amm, mint_a, mint_b)`, a pool-authority PDA, an LP mint, and two pool-owned ATAs for A and B. |
| `deposit_liquidity(amount_a, amount_b)` | LP (liquidity provider) | Moves A and B into the pool, mints LP tokens to the LP proportional to `sqrt(a * b)` (with a minimum-liquidity lock on the first deposit). |
| `withdraw_liquidity(amount)` | LP | Burns `amount` LP tokens and sends proportional A and B back to the LP. |
| `swap_exact_tokens_for_tokens(swap_a, input, min_output)` | trader | Trader sends `input` of one token, receives the computed output of the other, with slippage protection via `min_output`. A fee `fee` bps is deducted from `input` before the AMM maths. |

## 2. Glossary

**AMM (Automated Market Maker)**
: A smart contract that acts as a counterparty for token swaps. It
maintains token reserves and quotes prices from a formula
(bonding curve) rather than from an order book. Traders always have
a market, even when no human is on the other side.

**Constant-product formula (CPMM)**
: The simplest bonding curve: `x * y = k`, where `x` and `y` are
the pool's reserves of token A and token B, and `k` is a constant
set by the current liquidity. A trade must leave `k` unchanged (or
larger, with fees). Used by Uniswap V2.

**Basis points (bps)**
: 1 bp = 0.01%. A `fee = 30` means 0.30% fee per swap. `fee = 10000`
would be 100%, which this program rejects in `create_amm`.

**LP (Liquidity Provider)**
: Someone who deposits both tokens in the pool's current ratio,
receives LP tokens representing a share of the pool. Fees earned by
the pool accrue to LPs proportionally to their share.

**LP token (mint_liquidity)**
: A fresh SPL mint created per pool. `mint_authority` is the
pool-authority PDA. LP token supply grows when liquidity is added,
shrinks on withdrawal. Current share of the pool = your LP balance
/ total LP supply.

**Pool authority**
: A PDA seeded by `(amm, mint_a, mint_b, "authority")`. Owns the
two pool ATAs and is the mint authority for the LP mint. Signed by
the program using `invoke_signed` whenever tokens flow out of the
pool or LP tokens are minted/burned.

**Minimum liquidity**
: On the very first deposit, `MINIMUM_LIQUIDITY = 100` LP units are
"locked" (never minted to the depositor). Borrowed from Uniswap V2.
Prevents a first-depositor dust-attack where a tiny pool can be
manipulated by rounding.

**Invariant `k`**
: Post-swap, the product of the reserves must be `≥` the pre-swap
product (equal if fee is 0). The program recomputes `k` after every
swap and errors if it shrank.

**Slippage**
: The gap between the price you expected and the price you got. As
your input grows relative to reserves, `x * y = k` punishes you
more. `min_output_amount` lets you abort if slippage exceeds your
tolerance.

## 3. Accounts and PDAs

### `Amm` state

```
Amm { id: Pubkey, admin: Pubkey, fee: u16, bump: u8 }
```

### `Pool` state

```
Pool { amm: Pubkey, mint_a: Pubkey, mint_b: Pubkey, bump: u8 }
```

### PDAs

| PDA | seeds | owner | stores |
|---|---|---|---|
| `amm` | `[id]` | this program | `Amm` |
| `pool` | `[amm, mint_a, mint_b]` | this program | `Pool` |
| `pool_authority` | `[amm, mint_a, mint_b, "authority"]` | System | — (signer only) |
| `mint_liquidity` | `[amm, mint_a, mint_b, "liquidity"]` | SPL Token | LP mint (6 decimals, authority = pool_authority) |
| `pool_account_a` / `pool_account_b` | ATA(pool_authority, mint_a|b) | SPL Token | pool reserves |

## 4. Instruction lifecycle walkthrough

### `create_amm(id: Pubkey, fee: u16)`

**Who:** anyone. Caller becomes `admin`.

**Checks:** `fee < 10_000` (`InvalidFee`).

**State changes:** new `Amm` PDA with `{ id, admin = caller, fee,
bump }`.

### `create_pool()`

**Who:** anyone.

**Accounts:** `amm`, two mints A and B (with `mint_a < mint_b`
enforced by seed ordering; the constraint uses lexicographic
ordering to prevent creating both `(A, B)` and `(B, A)` pools).

**State changes:**
- New `Pool` PDA stores `{ amm, mint_a, mint_b, bump }`.
- `pool_authority` PDA derived.
- New LP mint with 6 decimals, authority = `pool_authority`,
  initial supply 0.
- Two ATAs created: `pool_account_a` (mint = A, owner =
  `pool_authority`) and `pool_account_b` (same for B).

### `deposit_liquidity(amount_a, amount_b)`

**Who:** any LP.

**Behaviour:**
1. Clamp `amount_a` and `amount_b` to the LP's actual balances
   (caps out at what they hold — so you can call with "max" values
   safely).
2. If this is the first deposit (`pool_a.amount == 0 && pool_b.amount
   == 0`), use the amounts as-is. Otherwise, adjust so the ratio
   matches the pool's: `amount_a' = amount_b * (pool_a / pool_b)`
   or vice versa (whichever side is the limiting factor).
3. `liquidity = sqrt(amount_a * amount_b)` using fixed-point
   arithmetic (`fixed::types::I64F64`).
4. If this is the first deposit:
   - If `liquidity <= MINIMUM_LIQUIDITY` → error
     `DepositTooSmall`.
   - Subtract `MINIMUM_LIQUIDITY` — those LP units are never
     minted, effectively locked forever.
5. Transfer `amount_a` A and `amount_b` B from depositor ATAs to
   pool ATAs.
6. CPI `mint_to` of `liquidity` LP tokens to the depositor's LP
   ATA, signed by `pool_authority`.

**Token movements:**

```
depositor_a --[amount_a]--> pool_account_a
depositor_b --[amount_b]--> pool_account_b
(mint LP)   --[sqrt(a*b) - MINIMUM_LIQUIDITY if first deposit]--> depositor LP ATA
```

### `withdraw_liquidity(amount)`

**Who:** LP holding LP tokens.

**Behaviour:**
1. Compute share:
   `share_a = amount / (supply + MINIMUM_LIQUIDITY) * pool_a.amount`,
   same for `share_b`.
2. Burn `amount` LP tokens from the LP's account.
3. Transfer `share_a` A and `share_b` B from the pool ATAs to the
   LP's ATAs. Pool authority signs via seeds.

**Token movements:**

```
pool_account_a --[share_a]--> lp ATA (mint A)
pool_account_b --[share_b]--> lp ATA (mint B)
(burn LP)  <--[amount]-- lp LP ATA
```

### `swap_exact_tokens_for_tokens(swap_a, input, min_output)`

**Who:** any trader.

**Behaviour:**
1. Clamp `input` to the trader's balance of the input token.
2. Apply fee: `taxed_input = input * (10_000 - fee) / 10_000`.
3. Compute output using constant product:
   ```
   If swap_a:  output = pool_b * taxed_input / (pool_a + taxed_input)
   Else:       output = pool_a * taxed_input / (pool_b + taxed_input)
   ```
   (Fixed-point `I64F64` for precision.)
4. If `output < min_output_amount` → error `OutputTooSmall`.
5. Record `k = pool_a * pool_b` pre-swap.
6. Transfer `input` to the pool, `output` from the pool to the
   trader. Pool authority signs for the outgoing leg.
7. Reload pool ATAs; check `new_a * new_b >= k`. If not, error
   `InvariantViolated`.

**Token movements (swap_a = true):**

```
trader ATA (A) --[input]--> pool_account_a
pool_account_b --[output]--> trader ATA (B)
```

**Checks:** `min_output`; invariant non-decrease; trader has enough
of the input token.

## 5. The constant-product invariant

For a pool with reserves `(x, y)` and trade input `Δx`:

```
  (x + Δx) * (y - Δy) = x * y    (ignoring fee)
→ Δy = y * Δx / (x + Δx)
```

Bigger `Δx` relative to `x` → worse marginal price. The curve never
lets reserves hit zero — the price asymptotes.

With a fee of `f` bps, only `Δx' = Δx * (1 - f/10000)` participates
in the formula, but the full `Δx` enters the pool. So `k` strictly
grows after every swap, which is how LPs earn yield.

## 6. Worked example

```
1. Alice calls create_amm(id = A1, fee = 30 bps).
   AMM stores { id: A1, admin: Alice, fee: 30, bump: <canonical> }.

2. Alice calls create_pool() for (USDC, WIF).
   Pool PDA, authority PDA, LP mint, and two pool ATAs created.
   Reserves: 0 USDC, 0 WIF. LP supply: 0.

3. Alice deposits 1_000 USDC + 10_000 WIF (she thinks they're
   worth the same in dollars).
   First deposit — ratio is whatever she says.
   liquidity = sqrt(1_000 * 10_000) = 3_162 (rounded, fixed point).
   Locked: 100. Minted to Alice: 3_062 LP.
   Pool: 1_000 USDC, 10_000 WIF.

4. Bob swaps 100 USDC for WIF.
   taxed_input = 100 * (1 - 0.003) = 99.7.
   output ≈ 10_000 * 99.7 / (1_000 + 99.7) ≈ 906 WIF.
   After: pool = 1_100 USDC, 9_094 WIF.
     k was 10_000_000; now it's 1_100 * 9_094 ≈ 10_003_400 (grew by
     fee revenue).

5. Alice withdraws all 3_062 LP tokens.
   share_a = 3_062 / (3_062 + 100) * 1_100 ≈ 1_065 USDC.
   share_b = 3_062 / (3_062 + 100) * 9_094 ≈ 8_806 WIF.
   She gets more USDC than she put in, less WIF. If the market
   price moved, she might gain or lose (impermanent loss).
```

## 7. Safety and edge cases

- **Front-running pool creation.** `create_pool` leaves the ratio
  of the first deposit to be whatever the first depositor picks.
  An attacker watching the mempool can race an honest first LP,
  seed the pool with a bad ratio, and arbitrage the real LP. Real
  AMMs mitigate this by making `create_pool` and the first deposit
  atomic or by restricting the first deposit.
- **Clamp-to-balance silently.** `deposit_liquidity` and
  `swap_exact_tokens_for_tokens` silently cap the input to the
  caller's balance. This is friendly ("send whatever I have") but
  means your slippage assumptions might be wrong if you didn't
  have the full amount. Test with tight `min_output` tolerances.
- **No `transfer_checked`.** Uses the older `token::transfer`,
  which doesn't validate mint/decimals on the accounts. A
  production AMM should use `transfer_checked`.
- **No `InvalidMint` enforcement.** The `TutorialError::InvalidMint`
  is defined but never emitted — the program relies on Anchor's
  `Box<Account<'info, Mint>>` and ATA constraints to catch
  mismatches. Probably fine for the simple case, but worth an
  explicit audit.
- **Fixed-point rounding.** Uses `I64F64`. Rounding favours the
  pool on every swap, which is the correct direction (LPs gain a
  bit, traders round down). Never the reverse.
- **Lexicographic mint ordering.** `create_pool` requires
  `mint_a.key() < mint_b.key()`. If a caller passes them swapped,
  the PDA derivation silently differs; the program relies on the
  seed constraint to reject. Clients should sort before calling.
- **Minimum liquidity lock.** The 100 LP tokens minted to nobody
  on pool creation are permanently stuck. Comes from Uniswap V2;
  prevents an attacker from donating tiny amounts to a fresh pool
  to skew the share calculation.
- **Fee bounds.** `create_amm` enforces `fee < 10_000` (100%).
  `fee = 9_999` technically legal — a 99.99% fee is functional
  but useless. Real AMMs cap this at something like 100 bps.

## 8. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Quasar
cd quasar && cargo test
```

The tests create an AMM, pool, deposit, swap, and withdraw, and
assert reserves / balances / LP supply at each step.

## 9. Extending the program

- **Cap fee at something sensible.** 0–100 bps is normal.
- **Use `transfer_checked`.** Cheap upgrade; catches mint/decimal
  mismatches.
- **Protect pool creation.** Combine `create_pool` + first
  `deposit_liquidity` into a single instruction, or require the
  first deposit to match an oracle price.
- **Concentrated liquidity.** Add tick ranges (à la Uniswap V3).
  Much more complex — different curve per tick, lots more state.
- **Multi-hop swaps.** A client-side batcher that chains swaps
  across pools; teaches you how swap ordering affects slippage.
- **Admin fees.** Route a fraction of the trading fee to the AMM
  admin's wallet. Introduces fee switches.
- **Oracle price feeds.** Export the current pool reserves as a
  time-weighted average price (TWAP) for other programs to read.
