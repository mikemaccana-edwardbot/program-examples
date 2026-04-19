# Checking accounts

A tour of how Solana programs validate the accounts they're handed.
Every account-check that more ambitious programs rely on comes from
the same small set of primitives — signer checks, ownership checks,
program-id checks, executability checks — and this example shows
the cheapest form of each.

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

Exposes one instruction (`check_accounts`) with a handler body that is
literally `Ok(())` — all the interesting work happens in Anchor's
`#[derive(Accounts)]` attributes before the handler runs. The
instruction takes four accounts, each annotated with a different
style of check, and rejects the transaction if any check fails.

If you've used Anchor before, the interesting bit is the difference
between (a) "I typed `Signer<'info>` and Anchor knows it has to
check the signer bit", (b) "I typed `UncheckedAccount<'info>` and I
have to either write a `CHECK:` comment or add a constraint", and
(c) "I typed `Program<'info, System>` and Anchor checks that the
pubkey is the System Program's and that the account is executable".

## 2. Glossary

**Signer check**
: The runtime records which of a transaction's keys actually
signed. An instruction that needs a signature on an account marks
the corresponding account-meta `is_signer = true`. Anchor's
`Signer<'info>` wrapper enforces this.

**Owner check**
: Every account has an `owner: Pubkey` field. A program that wants
to mutate an account must either own it or be calling via CPI on
someone who does. Anchor's `owner = <pubkey>` constraint generates
the check.

**Program-id check**
: Confirms a passed account is a specific program (matches a known
pubkey *and* is marked executable). `Program<'info, T>` does this
for any `T: Id` — for `System` it checks the System Program's id.

**Executable flag**
: An account-header bit set to true for deployed programs. Used by
`Program<'info, T>` to reject passing an arbitrary wallet in a
program slot.

**UncheckedAccount**
: Anchor's escape hatch: "no Anchor-generated checks — I'll do it
myself, or I deliberately want none". Requires a `/// CHECK:`
comment so the author has to think about what they're bypassing.

## 3. Accounts and PDAs

No PDAs, no new accounts. The four account slots are:

| Slot | Type | Check Anchor generates |
|---|---|---|
| `payer` | `Signer<'info>` | is_signer == true |
| `account_to_create` | `UncheckedAccount<'info>` (mut) | none (deliberate — demonstrates the bypass) |
| `account_to_change` | `UncheckedAccount<'info>` (mut, `owner = id()`) | owner equals this program's id |
| `system_program` | `Program<'info, System>` | pubkey == System Program id, executable |

## 4. Instruction lifecycle walkthrough

### `check_accounts`

**Who calls it:** any test harness wanting to see what's checked and
what isn't.

**Signers:** `payer`.

**Accounts in:** as above.

**Behaviour:** the handler is a no-op. All of the work is the
account-validation pass that Anchor runs *before* invoking the
handler:

1. Check `payer` signed.
2. Skip all checks on `account_to_create`.
3. Check `account_to_change.owner == program_id`.
4. Check `system_program.key() == solana_program::system_program::ID`
   and `system_program.executable == true`.

**Checks:** listed above.

**Token movements, state changes:** none.

## 5. Worked example

Pass four accounts:

- `payer`: any signer.
- `account_to_create`: any mutable pubkey (not actually mutated by
  this handler).
- `account_to_change`: must be owned by this program. Typically a
  test will create one via `SystemInstruction::CreateAccount` then
  assign the owner to the checking-accounts program id, then pass
  that pubkey here.
- `system_program`: `11111111111111111111111111111111`.

If any of these fail the pre-handler checks, the transaction
reverts with an Anchor error (e.g.
`ConstraintOwner`, `ConstraintSigner`).

## 6. Safety and edge cases

- **`UncheckedAccount` really is unchecked.** Nothing stops you
  passing, say, a foreign program's state account as
  `account_to_create` — the program body won't mind, but if
  production code read that account's data, it would be reading
  attacker-controlled bytes. Always pair an `UncheckedAccount`
  with an explicit constraint or an internal check.
- **`owner = id()` uses the program's own declared id.** If you
  transplant this code into another program and forget to re-run
  `anchor keys sync`, the constraint will check against the old id.
- **Executable-only `Program<'info, T>` check.** A non-executable
  account with the right pubkey fails the check (unlikely in
  practice but matters for audit trails).

## 7. Running the tests

```bash
cd anchor && anchor build && anchor test
cd native && cargo build-sbf && pnpm install && pnpm test
cd pinocchio && cargo build-sbf && cargo test
cd quasar && cargo test
```

## 8. Extending the program

- **Add `has_one = foo` between two accounts.** Shows how to check
  "this account's `foo` field equals that other account's pubkey" —
  the bread-and-butter of most PDA layouts.
- **Add a custom `constraint = ...` block** that runs arbitrary Rust
  predicates on the accounts before the handler.
- **Flip `account_to_create` to `#[account(init, ...)]`** so Anchor
  runs the creation CPI for you, then remove the unchecked slot.

## Framework differences

- **Anchor** — shown above; constraints drive validation.
- **Native** — checks are inline at the top of the handler: compare
  `key()`, `owner()`, `is_signer()` by hand.
- **Pinocchio** — same manual style as native with Pinocchio's
  lighter wrappers.
- **Quasar** — Anchor-compatible syntax on quasar-lang.
