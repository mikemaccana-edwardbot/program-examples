# Anchor Escrow

A two-party token swap with strict atomicity: Alice locks `X` of
token A into a vault and states "give me `Y` of token B in return";
Bob either fulfils it exactly and both sides settle, or the offer
stays open until Alice (or anyone in a future extension) cancels.

Nobody can run off with the other's tokens. Either the swap happens
in one transaction or neither party's holdings change.

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

Two instructions:

1. **`make_offer(id, token_a_offered_amount, token_b_wanted_amount)`**
   Maker creates an offer.
   - A fresh `Offer` PDA records the offer's metadata (the maker,
     the two mints, the wanted amount).
   - A fresh *vault* associated-token-account (owned by the `Offer`
     PDA) is created for `token_mint_a`.
   - `token_a_offered_amount` units of A move from the maker's ATA
     into the vault.

2. **`take_offer()`** Any taker fulfils an existing offer.
   - The taker sends `offer.token_b_wanted_amount` of B to the
     maker's ATA-B (creating it if necessary).
   - The vault's full balance of A (owned by the `Offer` PDA) is
     sent to the taker's ATA-A (creating it if necessary).
   - The empty vault is closed; its rent refunds to the taker.
   - The `Offer` PDA is closed (`close = maker`); its rent refunds
     to the maker.

If any step fails, the whole transaction reverts — neither side
loses anything beyond the tx fee.

## 2. Glossary

**Escrow** (tradfi analogy)
: In traditional finance, an escrow is a third party holding assets
until two counterparties each fulfil their side of a deal — e.g. a
title company holds a house deed until the buyer's funds clear.
Here the "third party" isn't a company; it's a PDA the program
signs for. The program code *is* the escrow rules, enforced by the
runtime.

**Maker / Taker**
: Borrowed from order-book parlance. The *maker* posts an offer
(locks tokens, publishes wanted amount). The *taker* fills it by
providing what the maker wanted. There's no price-improvement or
partial fill here — it's take-it-all-or-leave-it.

**Mint (SPL token)**
: An SPL token type. `token_mint_a` and `token_mint_b` are the two
token types in the swap. Every mint has an authority and a
decimals value.

**ATA (Associated Token Account)**
: The canonical token account for a given (mint, owner) pair.
Deterministic address: `find_program_address(["[owner]",
TOKEN_PROGRAM_ID, mint], ATA_PROGRAM_ID)`. Both parties' balances
live in ATAs.

**Vault**
: An ATA whose *owner* is the `Offer` PDA. Only the program can
move tokens out of it (by signing with the PDA's seeds). This is
the "held in escrow" part.

**PDA `Offer`**
: Seeds `["offer", maker.key(), id.to_le_bytes()]`. Deterministic
per (maker, id), so one maker can have many open offers
distinguished by `id` (a u64 the caller chooses). Stores:
```
id, maker, token_mint_a, token_mint_b, token_b_wanted_amount, bump
```

**`close = maker` (Anchor)**
: An account constraint that, at the end of the instruction,
transfers the account's lamports to `maker` and zeroes its data.
Reclaims rent.

**`transfer_checked`**
: The SPL-Token instruction that transfers tokens with an explicit
`mint` and `decimals`, rejecting if either disagrees with the
source/destination accounts. Safer than plain `transfer`.

**`Token-2022` / Token Interface**
: Anchor's `TokenInterface` and `InterfaceAccount` let the program
work with either SPL Token or Token-2022 (the newer programme with
extensions). The escrow doesn't care which — it just forwards the
`token_program` passed by the caller.

## 3. Accounts and PDAs

### `make_offer`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `maker` | signer, mut | SOL (pays rent + fee) | — | maker |
| `token_mint_a` | mint | — | A's mint config | — |
| `token_mint_b` | mint | — | B's mint config | — |
| `maker_token_account_a` | ATA (mint=A, owner=maker) | — | maker's A balance | — (maker signs via transfer) |
| `offer` | PDA, init | `["offer", maker, id_le]` | swap metadata (size 1+8+32+32+32+8+1 ≈ 114 bytes + discriminator) | program (via seeds) |
| `vault` | ATA (mint=A, owner=offer PDA), init | derived | A tokens held for the swap | program (via offer seeds, later) |
| `associated_token_program` | program | — | — | — |
| `token_program` | program (Token or Token-2022) | — | — | — |
| `system_program` | program | — | — | — |

### `take_offer`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `taker` | signer, mut | SOL + B | — | taker |
| `maker` | system account, mut | SOL (receives rent refund) | — | — |
| `token_mint_a` | mint | — | — | — |
| `token_mint_b` | mint | — | — | — |
| `taker_token_account_a` | ATA (mint=A, owner=taker), `init_if_needed` | — | will receive A | — |
| `taker_token_account_b` | ATA (mint=B, owner=taker) | — | taker's B balance | — (taker signs transfer) |
| `maker_token_account_b` | ATA (mint=B, owner=maker), `init_if_needed` | — | will receive B | — |
| `offer` | PDA, mut, `close = maker`, `has_one = maker/token_mint_a/token_mint_b` | `["offer", maker, id_le]` | swap metadata | program (via seeds) |
| `vault` | ATA (mint=A, owner=offer PDA) | derived | A tokens | program (via offer seeds) |

## 4. Instruction lifecycle walkthrough

### `make_offer(id, token_a_offered_amount, token_b_wanted_amount)`

**Who calls it:** the maker.

**Signers:** maker.

**Step by step:**

1. Anchor derives the `Offer` PDA from `["offer", maker, id_le]` and
   `init`s it (allocates, sets discriminator, payer = maker).
2. The vault ATA is created (mint = A, authority = `Offer` PDA).
3. `handle_send_offered_tokens_to_vault`:
   `transfer_checked(maker_ata_a → vault, amount =
   token_a_offered_amount, mint = A)`.
4. `handle_save_offer` writes the offer metadata (id, maker,
   mint_a, mint_b, `token_b_wanted_amount`, bump).

**Token movements:**

```
maker ATA (mint A) --[token_a_offered_amount]--> vault ATA (mint A, owner = Offer PDA)
```

**State changes:** new `Offer` PDA; new vault ATA; maker's A
balance decreases; vault's A balance = `token_a_offered_amount`.

**Checks:** `maker` signs; all ATAs are valid for their (mint,
owner) pairs; `token_program` matches both mints' token programs
(enforced by `mint::token_program = token_program`).

### `take_offer()`

**Who calls it:** any taker (no whitelist).

**Signers:** taker.

**Step by step:**

1. Anchor re-derives `Offer` PDA from seeds + stored bump. `has_one`
   constraints verify the passed `maker`, `token_mint_a`,
   `token_mint_b` match what the PDA stores.
2. `handle_send_wanted_tokens_to_maker`:
   `transfer_checked(taker_ata_b → maker_ata_b, amount =
   offer.token_b_wanted_amount, mint = B)`. Taker signs.
3. `handle_withdraw_and_close_vault`:
   - `transfer_checked(vault → taker_ata_a, amount = vault.amount,
     mint = A)`. The `Offer` PDA signs using `[b"offer", maker, id,
     bump]`.
   - `close_account(vault, destination = taker, authority = offer_PDA)`.
     Rent lamports return to the taker.
4. Anchor's `close = maker` constraint on `offer` closes it,
   returning rent to the maker.

**Token movements:**

```
taker ATA (mint B) --[token_b_wanted_amount]--> maker ATA (mint B)
vault ATA (mint A) --[vault.amount]--> taker ATA (mint A)
```

Plus rent movements: vault rent → taker; offer rent → maker.

**State changes:** `offer` closed; `vault` closed; taker's A
increases, taker's B decreases; maker's B increases; maker's A
unchanged (they already sent their A at `make_offer` time).

**Checks:** `has_one` on maker/mint_a/mint_b; vault ATA must be for
(mint_a, offer); taker's ATA-B and maker's ATA-B created if missing.

## 5. Worked example

```
Setup:
  Alice has 100 USDC, 0 WIF.
  Bob has 0 USDC, 1_000 WIF.

make_offer (Alice):
  id = 1, token_a_offered_amount = 10 USDC, token_b_wanted_amount = 100 WIF
  - Offer PDA created: seeds = ["offer", Alice, 1_le]
  - Vault ATA created for (USDC, Offer)
  - 10 USDC: Alice's USDC ATA -> Vault
  - Offer stores: { id:1, maker:Alice, mint_a:USDC, mint_b:WIF,
                    token_b_wanted_amount:100, bump:254 }
  - Balances:
      Alice USDC: 90, Alice WIF: 0
      Bob USDC:  0,  Bob WIF:  1000
      Vault USDC: 10

take_offer (Bob):
  - 100 WIF: Bob's WIF ATA -> Alice's WIF ATA (created if missing)
  - 10 USDC: Vault -> Bob's USDC ATA (created if missing)
  - Vault closed; rent -> Bob.
  - Offer closed; rent -> Alice.
  - Balances:
      Alice USDC: 90,  Alice WIF: 100
      Bob USDC:  10,  Bob WIF:   900
```

Both parties got exactly what was promised, atomically.

## 6. Safety and edge cases

- **All-or-nothing.** `take_offer` includes two transfers and two
  closes in one transaction. If any fails, the whole thing reverts.
  Alice can't lose her A without Bob losing B.
- **PDA signing.** Only the program, via the correct seeds, can
  withdraw from the vault. `invoke_signed` with `["offer", maker,
  id, bump]` is the only path.
- **Front-running.** A taker needs only `offer.token_b_wanted_amount`
  of B; anyone watching the mempool can race. The maker doesn't
  care *who* takes it, because the price is fixed. Not a bug, but
  worth noting.
- **No partial fills.** The vault's whole balance is always sent to
  the taker (`vault.amount` is the transfer amount). If you want
  partial fills, that's a bigger redesign.
- **`has_one` protections.** If Bob passes a different mint or a
  different maker than what `Offer` stores, Anchor rejects the tx
  before any transfer runs.
- **Mismatched token programs.** `mint::token_program =
  token_program` on the mint accounts enforces consistency. You
  can't mix SPL-Token and Token-2022 within one offer.
- **`init_if_needed` on taker's ATA-A and maker's ATA-B.** Comes
  with a gotcha: the taker pays the rent for *both* if the maker
  doesn't yet have an ATA-B. Cheap (~0.002 SOL) but the taker
  absorbs the cost.
- **No cancel.** The current program has no `cancel_offer`
  instruction. An abandoned offer leaves funds locked until
  someone takes it or you manually redeploy with new logic. (See
  "Extending".)
- **No `has_one = maker` on `MakeOffer`.** The maker is just the
  signer. The PDA seeds include the maker's pubkey, so a second
  caller with the same `id` creates a distinct PDA under their own
  pubkey.
- **ID collisions.** `seeds = ["offer", maker, id]` — if the maker
  reuses an `id` they already have an open offer for, `init` fails.

## 7. Running the tests

```bash
anchor build
anchor test
```

The test suite (`programs/escrow/tests/test_escrow.rs` for Rust,
and TypeScript tests in `tests/`) sets up two mints, two users,
makes an offer, takes it, and asserts the final balances. Anchor
spins up `solana-test-validator`.

## 8. Extending the program

- **Add `cancel_offer`.** Maker-signed instruction that transfers
  the vault balance back to the maker's ATA-A and closes the vault
  and the offer. Biggest missing feature; straightforward addition.
- **Expiry timestamp.** Store `expires_at: i64` on `Offer`; reject
  `take_offer` after that time; allow anyone to cancel expired
  offers (returning funds to maker). Uses `Clock::get()?.unix_timestamp`.
- **Partial fills.** Allow the taker to specify an amount less than
  the full vault; compute the proportional B to send based on the
  stored wanted amount. Much harder — you need to guard against
  rounding donations.
- **Whitelist.** Store `allowed_taker: Option<Pubkey>` on `Offer`; if
  `Some`, enforce in `take_offer`. Turns the open-order book into a
  P2P deal.
- **Fee to protocol.** Skim a basis-point fee off the A and B
  transfers into a protocol vault. Teaches you about integer
  rounding in token math.

## Credit

Based on [Dean Little's anchor-escrow-2024](https://github.com/deanmlittle/anchor-escrow-2024),
with renaming for clarity when used as teaching material (see
changelog in the commit history).
