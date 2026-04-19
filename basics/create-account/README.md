# Create Account

A program whose only job is to CPI into the System program to create
a fresh, empty, System-owned account. No PDAs, no state of its own.

It's the "hello world" of account creation: the minimum code needed
to allocate an address on Solana.

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

It exposes one instruction: `create_system_account`. When called, it
asks the System program (via CPI) to create a new account:

- owner set to the System program (not this program)
- zero bytes of data
- funded with enough lamports to be rent-exempt (`Rent::minimum_balance(0)`
  in anchor; exactly `1 SOL` in native — the native version slightly
  over-funds to keep the example simple)

The new account holds SOL and nothing else. Because its owner is the
System program, the only thing anyone can do with it is transfer
lamports out (which requires its signer).

## 2. Glossary

**System program**
: A built-in Solana program (address:
`11111111111111111111111111111111`) that owns every SOL wallet. It's
responsible for allocating new accounts, assigning owners, and
transferring lamports.

**CPI (Cross-Program Invocation)**
: One program calling another inside the same transaction. Here,
your program calls `system_program::create_account` on behalf of the
caller. See `basics/cross-program-invocation` for a deeper example.

**System account**
: An account owned by the System program. It can hold lamports but
no user data; writable operations on it must go through System
program instructions (transfer, allocate, assign).

**Rent-exempt**
: An account is rent-exempt when it holds at least
`Rent::minimum_balance(size)` lamports. Rent-exempt accounts are
never charged rent and won't be garbage-collected. Every production
account should be rent-exempt.

**Signer**
: An account whose private key signed the transaction. Creating a
new account at a given address requires *that address* to sign,
because the System program uses the signature as proof that the
address's owner authorises its use. (For PDAs, `invoke_signed`
substitutes a program-derived signature.)

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer | — | SOL to fund rent + tx fee | user |
| `new_account` | keypair, becomes System-owned | — | SOL; 0 bytes data | user (new keypair) |
| `system_program` | program | — | — | — |

No PDAs. Both `payer` and `new_account` are user-signed keypairs.

## 4. Instruction lifecycle walkthrough

### `create_system_account` (anchor) / default instruction (native)

**Who calls it:** anyone with SOL to fund the new account and the
transaction fee.

**Signers:** `payer` and `new_account`. The new account signs its
own creation.

**Accounts in:**
- `payer` (mut, signer)
- `new_account` (mut, signer) — the address to bring into existence
- `system_program`

**Behaviour:**
1. Log intent and the new address.
2. CPI to `system_program::create_account(from=payer, to=new_account,
   lamports, space=0, owner=system_program)`.
3. System program deducts `lamports` from `payer`, credits
   `new_account`, sets its owner, and finalises allocation.

**Token movements:**

```
payer (System-owned) --[lamports]--> new_account (System-owned)
```

**State changes:**
- `new_account` goes from "does not exist" to "exists, System-owned,
  0 bytes data".
- `payer.lamports -= lamports + tx_fee`.

**Checks:**
- Anchor requires both `payer` and `new_account` to be signers.
- The System program enforces that `new_account` isn't already
  created with a conflicting owner/size; if it exists with the same
  parameters and enough lamports, the instruction fails with
  `already in use`.

**Anchor vs native funding:**
- Anchor: `Rent::get()?.minimum_balance(0)` — the exact rent-exempt
  minimum for a zero-byte account (≈ 890 880 lamports on mainnet).
- Native: hard-coded `LAMPORTS_PER_SOL` (1 SOL). Wastefully generous,
  kept simple for pedagogy.

## 5. Worked example

```
1. Alice generates a fresh keypair K with address 9xF3...abc.
2. Alice submits a transaction with one instruction targeting this
   program, signed by Alice AND K, with:
     - payer = Alice
     - new_account = K
     - system_program = 111...111
3. Program logs:
     Program log: Program invoked. Creating a system account...
     Program log:   New public key will be: 9xF3...abc
4. Program CPIs into System::create_account.
5. System program:
     - Moves ~0.00089 SOL (anchor) / 1 SOL (native) from Alice to K
     - Sets K's owner to 111...111
     - Allocates 0 bytes of data at K
6. Program logs: "Account created succesfully."
7. Alice now has an empty System account at 9xF3...abc she controls.
```

She can do exactly two things with K from here:

- Transfer SOL out of K (requires K's signature).
- `Allocate`/`Assign` to give K to another program (requires K's
  signature) — which is what `basics/account-data` does next.

## 6. Safety and edge cases

- **`new_account` must sign.** Skipping the signer would let anyone
  grief by creating accounts at keys they don't control. Both
  variants enforce this; Anchor's constraint is
  `new_account: Signer<'info>`.
- **Account already exists.** If `new_account` is already created,
  the System program returns `SystemError::AccountAlreadyInUse` (code
  `0x0`). The tests re-roll a fresh keypair each run.
- **Insufficient payer lamports.** Fails early with
  `InsufficientFundsForRent` if the payer can't afford the rent +
  fee.
- **Wrong program owner requested.** This example sets owner =
  System program. If you set owner to something else, the System
  program still creates it, but only that owner can subsequently
  write to its data.
- **Racing the same keypair from two txs.** Only one wins; the
  second gets `AccountAlreadyInUse`. The client is expected to
  handle this.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native (program compiled with cargo build-sbf, tests via LiteSVM
# or TS against a local validator — see the native/ readme scripts)
cd native && cargo build-sbf && pnpm install && pnpm test
```

The tests exercise both paths:

- **CPI path:** client sends one transaction to *this* program, which
  then CPIs into the System program.
- **Direct path:** client talks straight to the System program. No
  custom program needed. Shown for comparison.

## 8. Extending the program

- **Allocate with data.** Pass `space > 0` and an owner pubkey to
  create a data account owned by another program (e.g. your own).
  Next stop: `basics/account-data`.
- **Derive the address.** Replace the keypair with a PDA and call
  `invoke_signed` so no extra keypair is needed. See
  `basics/pda-rent-payer` for PDA-as-payer and
  `basics/cross-program-invocation` for the CPI pattern.
- **Take lamports as an argument.** Accept an explicit `u64` for
  funding instead of hard-coding, so the caller can over-fund past
  rent-exempt.
- **Assign after creation.** Chain `allocate` + `assign` to hand the
  account to a different program in a second step.

### Links

- [Solana Cookbook — How to Create a System Account](https://solanacookbook.com/references/accounts.html#how-to-create-a-system-account)
- [Rust docs — `system_instruction::create_account`](https://docs.rs/solana-program/latest/solana_program/system_instruction/fn.create_account.html)
