# Rent

A program that creates a System-owned account big enough to hold an
`AddressData { name, address }` struct, computes the exact
rent-exempt minimum for that size, and transfers that many lamports
into the new account.

The purpose is to show the formula — storage size → rent-exempt
lamports — and how a program reads it at runtime.

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

One instruction: `create_system_account(address_data)`.

1. Serialise `address_data` to borsh to measure its byte length.
2. Ask the Rent sysvar for the rent-exempt minimum balance at that
   size.
3. CPI into `system::create_account(from=payer, to=new_account,
   lamports=<minimum>, space=<length>, owner=system_program)`.

The new account ends up System-owned. Note that although it's sized
for the `AddressData`, the program never actually *writes* the data
into it — it just uses the struct to compute the size. (A real
version would allocate under the program's own ownership and write
the borsh bytes; see "Extending".)

## 2. Glossary

**Rent**
: The lamport cost of storing data on Solana, proportional to
account size. Rent *can* be collected per-epoch from non-exempt
accounts (eventually garbage-collecting them), but in practice every
account on the network is rent-exempt — anyone allocating a
non-exempt account has their transaction rejected at the runtime
level. So in practice: pay the minimum balance once, keep it forever,
pay no ongoing fees.

**Rent-exempt minimum**
: The lamport balance an account must hold to be exempt. Formula:
`DEFAULT_LAMPORTS_PER_BYTE_YEAR * (ACCOUNT_STORAGE_OVERHEAD + size) * DEFAULT_EXEMPTION_THRESHOLD`.
Concretely on current Solana it's about `0.00000348 SOL` per byte
plus a 128-byte overhead contribution, so a 0-byte account costs
~0.00089 SOL. You never have to remember the formula — call
`Rent::get()?.minimum_balance(size)`.

**Rent sysvar**
: A special account at address `SysvarRent111111...111` that exposes
the current rent parameters. Programs read it via
`Rent::get()` (which internally uses `solana_program::sysvar::rent::Rent`).

**Borsh length**
: Borsh serialises dynamically-sized types like `String` with a
4-byte length prefix followed by the data. The program measures
`borsh::to_vec(&address_data)?.len()` to know how many bytes to
allocate — this includes the length prefixes.

**System-owned vs program-owned**
: This example creates a *System-owned* account. It has storage
allocated but only the System program can mutate it. For a useful
data account, you'd pass `owner = program_id` so *this* program can
write to it later.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer, mut | SOL (pays rent + fee) | — | user |
| `new_account` | keypair, mut, signer | becomes System-owned account of size `|borsh(address_data)|` | user (keypair) |
| `system_program` | program | — | — | — |

No PDAs. No program-owned data account.

## 4. Instruction lifecycle walkthrough

### `create_system_account(address_data)`

**Who calls it:** anyone with a fresh keypair.

**Signers:** `payer`, `new_account`.

**Accounts in:**
- `payer` (signer, mut)
- `new_account` (signer, mut) — not yet created
- `system_program`

**Behaviour:**
1. `let account_span = borsh::to_vec(&address_data)?.len();`
2. `let lamports_required = Rent::get()?.minimum_balance(account_span);`
3. Logs:
   ```
   Account span: <N>
   Lamports required: <L>
   ```
4. CPI `system::create_account(from=payer, to=new_account,
   lamports=L, space=N, owner=system_program)`.

**Token movements:**

```
payer --[L lamports]--> new_account (System-owned, N bytes)
```

**State changes:** `new_account` goes from "doesn't exist" to
"System-owned, N bytes, L lamports". The account data remains all
zeroes — the program never writes `address_data` into it.

**Checks:** both `payer` and `new_account` must sign (enforced by
Anchor). System program rejects the CPI if `new_account` already
exists.

## 5. Worked example

```
1. Alice builds address_data = {
     name: "Alice Anderson",
     address: "221B Baker Street"
   }

2. Borsh length:
     4  (name len)
   + 14 (name bytes)
   + 4  (address len)
   + 17 (address bytes)
   = 39 bytes

3. Rent::get().minimum_balance(39):
     ≈ 1_280_880 lamports
     (0 bytes = 890_880; +39 bytes × ~6960 lamports/byte)

4. Alice submits tx signed by Alice + new_keypair.
   Program logs:
     Account span: 39
     Lamports required: 1280880
     Account created succesfully.

5. new_keypair now exists: 39 bytes (all zero), System-owned,
   1_280_880 lamports balance.
```

At any point Alice can close the account by having `new_keypair`
sign a transfer of all its lamports out — System program accounts
can be drained freely by their signer.

## 6. Safety and edge cases

- **The data isn't written.** This is a pedagogical gap: the program
  allocates the space but never writes the borsh bytes. The account
  ends up size-N but empty. To actually store the struct you need
  `owner = program_id` (so this program may write) plus a
  `data.borrow_mut()[..].copy_from_slice(&borsh_bytes)` step.
- **`new_account` must sign.** Creation always requires the target
  address to sign, as a proof-of-control. Using a PDA would replace
  this with `invoke_signed` + seeds (see `basics/pda-rent-payer`).
- **Insufficient payer balance.** System program returns
  `InsufficientFundsForRent`.
- **Over-long strings.** `String` has no hard cap, but the full
  transaction (all instruction data + signatures + accounts) must
  fit in ~1232 bytes. Practically this bounds name + address to a
  few hundred bytes each.
- **Rent-epoch edge case.** A rent-exempt account remains exempt
  forever, even if you later add more data (through `realloc`) —
  *as long as* its lamport balance still meets the new minimum. If
  the lamports drop below, the account becomes rent-paying and
  eventually eligible for deletion.

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

The tests submit a `create_system_account` instruction, fetch the
new account, and assert both its size and that it's rent-exempt
(`lamports >= Rent::minimum_balance(data_len)`).

## 8. Extending the program

- **Actually write the data.** Change `owner = program_id`, allocate
  the right space, and have the program `borsh-serialise` the
  `address_data` struct into `new_account.data`. Now you've got a
  read-back-able data account.
- **Use `realloc`.** Start the account small; expand it in a second
  instruction (top up rent at the same time).
- **Overallocate intentionally.** Size the account larger than
  `address_data` to leave room for future fields. Shows that rent
  scales with `space`, not with actual used bytes.
- **Print the formula.** Add a read-only instruction that logs
  `rent_exempt_minimum` for a given hypothetical size, so clients
  can compute costs without simulating a tx.
- **Non-exempt account.** Allocate with `lamports < minimum_balance`
  and watch the tx fail with `InsufficientFundsForRent` — the
  runtime forbids making new non-exempt accounts as of SIMD-0085.
