# Close account

Two instructions: `create_user` spins up a per-user PDA with a name
field, `close_user` tears it back down and refunds the lamports. The
"close" half is what you're here to see — it shows Anchor's `close =
<dest>` constraint, which is the standard way to recycle an account's
rent-exempt reserve.

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

- `create_user(name)` — derives a PDA at seeds `["USER", user]`,
  initialises it to `UserState { bump, user, name }`, pays the
  rent-exempt reserve out of the `user` signer.
- `close_user()` — re-derives the same PDA (checking the stored
  bump), marks it for closure, and returns its lamports to the
  `user`. Anchor's `close = user` constraint does the work: it sets
  the account's data to zero, zeroes its data length, and transfers
  all lamports out. From the runtime's perspective, the account is
  now a system-owned, zero-lamport, zero-data account — eligible
  for garbage collection on the next slot.

## 2. Glossary

**PDA (Program Derived Address)**
: An address deterministically derived from seeds + a program id.
Has no private key. The program "signs" as a PDA by re-supplying
the seeds during a CPI.

**Seeds**
: The byte strings that derive a PDA. Here: `[b"USER", user.pubkey]`.

**Bump**
: The one-byte offset that makes the seed-derivation land on an
off-curve address. Stored on the account so the program doesn't
have to re-call `find_program_address` (256 iterations worst case)
every instruction.

**Rent-exempt reserve**
: The lamport balance an account needs to exist at its data length
without the runtime reaping it. `close_user` returns exactly this
amount (plus any surplus) to the `user`.

**close = \<dest\>**
: Anchor constraint on an account field. Zeroes the account's data,
zeroes its discriminator, then drains its lamports into `dest`.
Applies in the pre-handler pass (before your handler body runs),
but takes effect at the end (so your handler can still read the
data).

## 3. Accounts and PDAs

| Account | PDA? | Seeds | Owner | Holds |
|---|---|---|---|---|
| `user` | no | — | System Program | Pays rent on create; receives it back on close |
| `user_account` | yes | `["USER", user]` | program | `UserState { bump, user, name (max 50) }` |

## 4. Instruction lifecycle walkthrough

### 4.1 `create_user(name)`

**Signers:** `user`.

**Token movements:**
```
user (lamports) --[rent-exempt for ~91 bytes]--> user_account
```

**State changes:** new `UserState` record written with bump, user,
name.

**Checks:** Anchor `init` (account uninitialised) + `seeds`/`bump`
derivation.

### 4.2 `close_user()`

**Signers:** `user` (the same one whose pubkey was used as a seed).

**Token movements:**
```
user_account (lamports) --[full balance]--> user
```

**State changes:** `user_account.data` zeroed, discriminator zeroed
(via `close` constraint). The runtime garbage-collects the account
on the next slot.

**Checks:**
- `user_account` is the PDA seeded by `["USER", user]` — Anchor's
  `seeds` + `bump = user_account.bump` constraint.
- `user` is a signer.
- The `close = user` constraint requires the destination to be
  writable (`mut`); the `user` field is.

## 5. Worked example

1. Alice calls `create_user("Alice")`. A new account at
   `PDA(["USER", alice])` exists, holding `UserState { bump, user:
   alice.pubkey, name: "Alice" }`. Alice's wallet is down by the
   rent-exempt reserve (~0.001 SOL) + the transaction fee.
2. Alice calls `close_user()`. The PDA is drained into Alice's
   wallet. She recovers the rent-exempt reserve minus this
   transaction's fee.

## 6. Safety and edge cases

- **Only `user` can close.** Because the PDA seeds include
  `user.key()`, no other signer derives the same PDA and the
  seeds-check fails.
- **`close =` sets the data to zero before the handler returns.**
  If your handler needs to read fields after the close constraint
  triggers, you'll get zeros. Solution: read them *into local
  variables* at the top of the handler.
- **A closed account can be "reopened" in the same transaction.**
  Anchor's `close` writes a zero-discriminator sentinel, but the
  account still exists until the end of the transaction. This
  mostly matters for the CPI-into-yourself edge case; see the
  Anchor docs on "reinit attacks".

## 7. Running the tests

```bash
cd anchor && anchor build && anchor test
cd native && cargo build-sbf && pnpm install && pnpm test
cd pinocchio && cargo build-sbf && cargo test
cd quasar && cargo test
```

## 8. Extending the program

- **Delegate the authority.** Split `user` (whose pubkey seeds the
  PDA) from `payer` (who gets the refund). Useful when a keeper
  wants to clean up stale accounts.
- **Accumulate state before close.** Add an `update_user(name)` that
  lets the user rename before closing. Exercises mut-borrow on the
  Anchor account wrapper.
- **Close on condition.** Add a timestamp field; `close_user` checks
  it's past some TTL before allowing close. Shows how to gate close
  behaviour on on-chain state.

## Framework differences

- **Anchor** — `close = user` constraint does the bookkeeping.
- **Native** — close is manual: move lamports with
  `try_borrow_mut_lamports`, then zero the data length with
  `realloc(0, false)` and set the owner to the System Program.
- **Pinocchio** — same manual style with Pinocchio's account helpers.
- **Quasar** — Anchor-compatible surface on quasar-lang.
