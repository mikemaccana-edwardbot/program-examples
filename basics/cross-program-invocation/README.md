# Cross-Program Invocation (CPI)

Two programs: `lever` (owns a single bool account, `PowerStatus`,
with a `switch_power` instruction that toggles it) and `hand` (has
one instruction, `pull_lever`, that CPIs into the lever program).

The purpose is to show a tiny, self-contained CPI: how to declare
the callee program in your accounts, build a `CpiContext` /
`Instruction`, and let the runtime hand control to another program
mid-transaction.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [Worked example](#5-worked-example)
6. [Safety and edge cases](#6-safety-and-edge-cases)
7. [Running the tests](#7-running-the-tests)
8. [Framework differences](#8-framework-differences)
9. [Extending the program](#9-extending-the-program)

## 1. What does this program do?

- **`lever`** owns `PowerStatus { is_on: bool }`. It has
  `initialize` (creates a new `PowerStatus` account) and
  `switch_power(name: String)` (flips the bool, logs who flipped
  it).
- **`hand`** has `pull_lever(name: String)`. It builds a CPI to
  `lever::switch_power` and forwards the name.

Outcome of calling `hand::pull_lever(name)`:

```
lever.PowerStatus.is_on ^= 1    # toggled
log: "<name> is pulling the power switch!"
log: "The power is now on." | "The power is now off!"
```

## 2. Glossary

**Cross-Program Invocation (CPI)**
: One program calling another within the same transaction. The
callee runs with its own program id, its own `AccountInfo`s, but
shares the transaction's compute budget and signers.

**Signer (inside a CPI)**
: A signature granted by the outer transaction carries into a CPI
automatically. A program can also "sign" for PDAs it owns using
`invoke_signed` + seeds. This example doesn't use
`invoke_signed` — the `PowerStatus` account is already authored
and writable, no PDA signing needed.

**`CpiContext` (Anchor)**
: A small wrapper around (program id, accounts, optional signer
seeds) used by Anchor's generated CPI helpers. `CpiContext::new(...)`
for unsigned CPIs; `::new_with_signer(..., seeds)` for PDA CPIs.

**`declare_program!` (Anchor)**
: A macro that reads a foreign program's IDL (placed in `idls/`) and
generates typed CPI stubs — `lever::cpi::switch_power`,
`lever::cpi::accounts::SwitchPower`, etc.

**`no-entrypoint` feature (Native)**
: A Cargo feature trick to import one Solana program crate into
another without linking two `entrypoint!`s. Building `lever` as a
library from within `hand` enables the feature to suppress the
entrypoint macro. See "Native gotcha" in §8.

**Borsh**
: The binary serialisation format Solana uses by convention. The
native variant builds the CPI instruction with
`Instruction::new_with_borsh`, borsh-serialising the argument
struct into the instruction data bytes.

## 3. Accounts and PDAs

No PDAs. Every account is a plain keypair or signer.

### `lever::initialize`

| name | kind | stores | who signs |
|---|---|---|---|
| `power` | keypair, init | `PowerStatus { is_on: bool }` | keypair (at creation) |
| `user` | signer, mut | SOL (pays rent) | user |
| `system_program` | program | — | — |

### `lever::switch_power`

| name | kind | stores | who signs |
|---|---|---|---|
| `power` | mut | `PowerStatus` | — (no signer required) |

### `hand::pull_lever`

| name | kind | stores | who signs |
|---|---|---|---|
| `power` | mut | `PowerStatus` (passed through to lever) | — |
| `lever_program` | program | — | — |

## 4. Instruction lifecycle walkthrough

### `lever::initialize`

Creates a fresh `PowerStatus` account owned by the lever program,
sized `discriminator + bool`. `user` pays rent.

### `lever::switch_power(name)`

Toggles `power.is_on`, logs two messages. Nothing else.

### `hand::pull_lever(name)` — the CPI

1. Caller submits a transaction with one instruction targeting `hand`.
   Accounts: `power`, `lever_program`.
2. `hand` constructs a CPI to `lever::switch_power`:
   - **Anchor path:** `switch_power(CpiContext::new(lever_program,
     SwitchPower { power }), name)`. Anchor's generated client does
     the discriminator, borsh serialisation, and
     `invoke(&ix, &[power])` for you.
   - **Native path:** builds an `Instruction` by hand with
     `Instruction::new_with_borsh(lever_program_id,
     &SetPowerStatus { name }, vec![AccountMeta::new(power, false)])`
     then `invoke(&ix, &[power.clone()])`.
3. The runtime transfers control to `lever`. Its
   `process_instruction` deserialises the bytes, falls through the
   `SetPowerStatus` arm, and flips `is_on`.
4. Control returns to `hand`, which returns `Ok(())`.

**Call graph:**

```
tx signed by user
 └── hand::pull_lever(name)
      └── invoke(Instruction{target=lever, data=SetPowerStatus{name}, accounts=[power]})
           └── lever::switch_power(name)
                └── mutates power.is_on
```

**State changes:** `power.is_on = !power.is_on`.

**Token movements:** none at `switch_power` / `pull_lever` time.
`initialize` transfers rent lamports from `user` to `power`.

## 5. Worked example

```
1. Alice calls lever::initialize with keypair K for the power account.
   - power.is_on = false
   - Alice pays rent (~0.00089 SOL for bool + 8-byte discriminator)

2. Alice calls hand::pull_lever("Alice"), passing power=K, lever_program=lever.
   Logs:
     Program <hand> invoke [1]
     Program <lever> invoke [2]
     Program log: Alice is pulling the power switch!
     Program log: The power is now on.
     Program <lever> success
     Program <hand> success
   power.is_on = true.

3. Bob calls hand::pull_lever("Bob"), power=K.
   Logs:
     Bob is pulling the power switch!
     The power is now off!
   power.is_on = false.
```

Note that in step 3, Bob didn't initialise or own `K` — the lever
program intentionally has no access control on `switch_power`.
Anyone with a writable reference to the account can toggle it.

## 6. Safety and edge cases

- **No access control on `switch_power`.** Any signer can flip the
  switch. For a real program you'd add an `authority` field on
  `PowerStatus` and constrain it.
- **Account owner check.** Anchor verifies `power` is owned by
  `lever` via the account discriminator. In native code, `lever`
  deserialises `PowerStatus::try_from_slice(&power.data)` which
  accepts any 1-byte data — so a malicious caller could pass an
  attacker-owned account with fabricated `is_on` bytes. A production
  native program must check `power.owner == program_id`.
- **CPI depth.** Solana caps CPI depth at 4 levels. Here we go
  depth 2 (client → hand → lever), well under the limit.
- **Compute budget.** The CPI shares the transaction's 200 000 CU.
  Tiny programs like this consume <5 000 CU.
- **Instruction data parsing order (native lever).** `lever` tries
  `PowerStatus::try_from_slice` first, then `SetPowerStatus`. A
  `PowerStatus { is_on: bool }` serialises to a single byte (`0x00`
  or `0x01`). A `SetPowerStatus { name }` starts with a 4-byte
  little-endian length prefix. Collision is unlikely but this
  dispatch style (try-decode) is fragile — in production you'd use
  an explicit discriminant byte (as the `counter` native variant
  does).
- **`no-entrypoint` feature required.** If `hand`'s `Cargo.toml`
  imports `lever` without the `no-entrypoint` feature, you'll get
  a link error ("multiple definitions of `entrypoint`"). See §8.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar/lever && quasar build && cd ../hand && quasar build && cargo test
```

The tests initialise `PowerStatus`, call `hand::pull_lever` twice,
and assert `is_on` ends up `false` again after two flips.

## 8. Framework differences

| Variant | CPI helper | IDL handling | Notes |
|---|---|---|---|
| `anchor/` | `CpiContext::new(...) + switch_power(ctx, name)` | `declare_program!(lever)` reads `idls/lever.json` and generates typed stubs | Checks account owner automatically |
| `native/` | `Instruction::new_with_borsh(...) + invoke(...)` | None — the callee's `SetPowerStatus` struct is imported directly as a Rust type | Requires `no-entrypoint` Cargo feature |
| `quasar/` | `BufCpiCall` — build discriminator + borsh by hand | None — uses a marker type implementing `Id` | Lowest-level; explicit wire format |

### Native gotcha: `no-entrypoint`

A Solana program crate has exactly one `entrypoint!(...)`. Importing
one program crate into another would try to link two, which the
linker rejects. The fix is a Cargo feature:

In the callee (`lever`) `Cargo.toml`:

```toml
[features]
no-entrypoint = []
```

And guard the macro:

```rust
#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);
```

In the caller (`hand`) `Cargo.toml`:

```toml
[dependencies]
lever = { path = "../lever", features = ["no-entrypoint"] }
```

Now the `entrypoint!` compiles when `lever` builds standalone, and
disappears when `hand` imports it.

### Quasar gotcha: no `declare_program!`

Quasar doesn't have an IDL-driven CPI generator yet. The `hand`
program declares a marker type implementing `Id` with the lever's
address, uses `Program<LeverProgram>` in accounts (for compile-time
address + executable checks), and builds the wire format manually
with `BufCpiCall`. More verbose than Anchor, but no IDL juggling.

## 9. Extending the program

- **Add access control.** Store `authority: Pubkey` on
  `PowerStatus`; have `switch_power` check `ctx.accounts.authority ==
  stored_authority`. Forces the hand program to forward a signer in
  the CPI.
- **`invoke_signed` with a PDA.** Make `power` a PDA owned by
  `lever`, and have `hand` sign for it in the CPI using seeds only
  `hand` knows. Demonstrates delegated write authority via PDA.
- **Return data.** Use `set_return_data` / `get_return_data` in
  `switch_power` so `hand` can read the new `is_on` after the CPI.
- **Chain a third program.** Add a `finger` program that calls
  `hand` which calls `lever`. Shows how CPI depth accumulates
  (limit: 4).
- **Replace CPI with direct client call.** Put two instructions in
  one transaction from the client: `hand::log_something` +
  `lever::switch_power`. Same net effect, no CPI — a useful
  comparison for when CPI is worth it.
