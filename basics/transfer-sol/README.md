# Transfer SOL

Two ways for a Solana program to move SOL (lamports) from one account
to another, shown side by side: calling the System Program via a CPI,
and directly editing lamport balances on accounts the program owns.

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

The program exposes two instructions, each moving `amount` lamports
from one account to another:

- **`transfer_sol_with_cpi`** — the *payer* is a System-Program-owned
  wallet (a normal keypair). The program can't edit lamports on it
  directly, so it calls the System Program's `Transfer` instruction
  (a CPI) and lets the runtime enforce the usual signer rules. This
  is the standard pattern.

- **`transfer_sol_with_program`** — the *payer* is an account owned
  by this program. The runtime lets a program directly mutate
  lamports on accounts it owns, so the handler just debits one
  balance and credits the other with a pair of borrows. Faster (no
  CPI) but only works because of the ownership rule.

Both instructions take `(payer, recipient, amount)`; the second one
also implicitly requires the System Program as a program in the
transaction for Anchor's validation machinery.

## 2. Glossary

**Lamport**
: The smallest unit of SOL. 1 SOL = 10⁹ lamports.

**System Program**
: A built-in Solana program (id `11111111111111111111111111111111`) that
owns every normal wallet account and handles operations on them:
creating accounts, allocating space, transferring SOL, assigning
ownership to a different program. Any time you want to move lamports
out of an account that you, the program, don't own, you have to go
through the System Program.

**Account owner**
: Every Solana account has an `owner` field — the pubkey of the
program allowed to mutate its data and lamports. Most wallets are
owned by the System Program. Accounts created by a program via
`invoke_signed` or Anchor's `init` are owned by that program.

**CPI (Cross-Program Invocation)**
: One program calling another in the same transaction. The CPI passes
account metas and instruction data; the called program runs with
its own privilege (PDA signatures etc.). `transfer_sol_with_cpi`
uses a CPI to the System Program.

**Signer**
: An account whose private key signed the transaction. Required by
the System Program to authorise a transfer from a system-owned
account.

**try_borrow_mut_lamports**
: Rust helper on `AccountInfo` that returns a mutable reference to
an account's lamport balance. Only legal on accounts the program
owns (the runtime checks this at the end of the instruction). Used
by `transfer_sol_with_program`.

## 3. Accounts and PDAs

No PDAs are created. The program touches only pre-existing accounts:

| Account | Kind | Why |
|---|---|---|
| `payer` | System-owned wallet (CPI variant) or program-owned account (direct variant) | Source of the lamports |
| `recipient` | `SystemAccount` | Destination |
| `system_program` | Program | Required by Anchor in the CPI variant; optional in the direct variant |

## 4. Instruction lifecycle walkthrough

### 4.1 `transfer_sol_with_cpi`

**Who calls it:** anyone who has signed with the `payer`.

**Signers:** `payer`.

**Accounts in:** `payer` (signer, mut), `recipient` (mut),
`system_program`.

**Behaviour:**

```
payer --[amount lamports]--> recipient        (via System Program CPI)
```

**Checks:** Anchor's `Signer<'info>` ensures `payer` signed. The
System Program enforces that `payer` has enough lamports and that it
owns the account.

### 4.2 `transfer_sol_with_program`

**Who calls it:** any transaction where both accounts are passed.
The `payer` doesn't need to sign — because the program owns it, the
runtime permits direct lamport edits.

**Signers:** none required (although at least one signer is always
needed on the transaction itself for fee payment).

**Accounts in:** `payer` (unchecked, owned by the program, mut),
`recipient` (SystemAccount, mut).

**Behaviour:**

```
payer.lamports  -= amount
recipient.lamports += amount
```

**Checks:** `owner = crate::ID` Anchor constraint enforces that
`payer`'s owner is this program. The runtime post-check will reject
the transaction if the total lamports across accounts don't balance
or if non-owned account lamports changed.

## 5. Worked example

Call `transfer_sol_with_cpi(100_000)` with:

- `payer` = Alice's wallet (owned by System Program), balance 1 SOL
- `recipient` = Bob's wallet, balance 0
- Alice signs

After the transaction: Alice 999 999 900 lamports (minus a small
transaction fee), Bob 100 000 lamports.

For `transfer_sol_with_program(100_000)`, `payer` must be an account
this program owns — e.g. a program-derived account that was
previously funded. No signer is required for the `payer`, because
the program has authority over its lamports.

## 6. Safety and edge cases

- **Don't mutate lamports on accounts you don't own.** The runtime
  does a post-instruction check and will reject the transaction.
  `transfer_sol_with_program` uses the `owner = crate::ID`
  constraint to make this explicit.
- **Always leave rent-exempt minimums intact.** Draining an account
  below its rent-exempt reserve for its current data size puts the
  account in a state that the runtime may garbage-collect. For
  data-less accounts this isn't a concern, but a program-owned
  account with 100 bytes has a minimum.
- **Overflow / underflow**: subtracting `amount` from `payer`'s
  lamports with `-=` on a `u64` will panic on underflow in debug
  builds and wrap in release. For production code, prefer
  `checked_sub` with a proper error. This example trades that
  rigour for readability.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Pinocchio
cd pinocchio && cargo build-sbf && cargo test

# Quasar
cd quasar && cargo test
```

CI (`.github/workflows/anchor.yml` etc.) runs the same commands.

## 8. Extending the program

- **Take a vault PDA as the `payer`.** Shows how a program can
  "hold" SOL in a PDA account and release it via signer seeds. See
  `basics/pda-rent-payer` for a nearby example.
- **Add a per-recipient cap.** Track a PDA per recipient with a
  lifetime total and reject transfers over a threshold.
- **Transfer via `invoke_signed`.** Change the CPI path to use
  `invoke_signed` with PDA seeds so a PDA can be the sender in the
  first variant.

## Framework differences

- **Anchor** — uses `CpiContext` + `system_program::transfer` for the
  CPI variant; `try_borrow_mut_lamports` for the direct variant.
  `#[account(owner = crate::ID)]` enforces ownership on the direct
  variant.
- **Native** — same two patterns, spelled without Anchor macros.
  The direct variant reads lamports via `**account.try_borrow_mut_lamports()?`.
- **Pinocchio** — same two patterns with Pinocchio's allocator-free
  entrypoint macros.
- **Quasar** — mirrors Anchor's surface but compiles against the
  smaller quasar-lang runtime.
