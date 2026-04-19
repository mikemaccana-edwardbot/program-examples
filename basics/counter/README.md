# Counter

A one-field account (`count: u64`) that anyone can increment by one.
There are five implementations — anchor, native, pinocchio, mpl-stack
and quasar — all doing the same onchain thing with slightly different
ergonomics.

The purpose is to see the smallest non-trivial program: one account
of state, one mutation, and enough framework surface to pick one for
your own projects.

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

Two things, both trivial:

1. **Create a counter account** owned by the program, storing a
   single `u64` initialised to zero.
2. **Increment** that counter by one. Any signer can call this
   against any existing counter.

That's it. No access control on the increment (intentional — see the
"Safety" section), no decrement, no max value.

## 2. Glossary

**Account**
: Any 32-byte-addressed blob of state on Solana. Has an owner
program, lamport balance, data bytes, and an `is_executable` flag.
The counter here is a data account: 8 bytes (plus framework
overhead) holding the counter value.

**PDA (Program-Derived Address)**
: An address derived deterministically from a set of seeds and the
program id. PDAs have no private key, so only the owning program can
"sign" for them using `invoke_signed`. Useful for accounts whose
address you want to rediscover later from known inputs. The Anchor,
native, pinocchio and mpl-stack variants use a plain keypair for the
counter; the quasar variant uses a PDA seeded by `"counter"` +
payer.

**Keypair account**
: An account at an address with a known private key (a fresh
keypair). The user signs its creation and then the program becomes
its owner. Simple but means you need to remember the address
externally.

**Discriminator**
: A short (usually 8-byte) prefix at the start of an account's data
that identifies the account type. Anchor and Quasar add one
automatically; the native and pinocchio variants here skip it and
store raw borsh'd bytes.

**Instruction discriminant**
: A byte (or a few bytes) at the start of instruction data that
tells the program which handler to run. Native / pinocchio /
mpl-stack use the first byte (`0x00 = increment`). Anchor uses the
8-byte hash-based discriminator of the method name. Quasar lets you
pick (the example uses `0` and `1`).

**Rent**
: Lamports held by every account, proportional to its size, required
to stay rent-exempt. The payer funds the counter account at
creation.

**Borsh**
: A deterministic binary serialisation format used across Solana. The
native and mpl-stack variants serialise the `Counter` struct as
borsh. Anchor and quasar generate the serde for you.

## 3. Accounts and PDAs

The single account differs slightly between variants:

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `counter` (anchor / native / pinocchio / mpl-stack) | keypair, program-owned | — | `u64 count` | payer + counter keypair (to create) |
| `counter` (quasar) | PDA, program-owned | `["counter", payer]` | `u64 count` (+ 1-byte discriminator) | payer (program signs via seeds) |
| `payer` | signer | — | native SOL | user |
| `system_program` | program | — | — | — |

Anchor's account carries an 8-byte discriminator automatically, so
space is `8 + 8 = 16` bytes. The native / pinocchio variants
allocate 8 bytes (just the u64). The `initialize_counter` step that
does the allocation is not implemented in the native variant — it
expects the client to create and pre-fund the account.

## 4. Instruction lifecycle walkthrough

### `initialize_counter` (anchor, quasar; implicit in others)

**Who calls it:** any wallet willing to pay rent.

**Signers:** payer, plus the new counter keypair (anchor only; in
quasar the PDA is "signed" by the program via its seeds).

**Accounts in:**
- `payer` (signer, mut) — funds the rent
- `counter` (mut, init) — new account to create
- `system_program` — needed by the runtime to allocate accounts

**Behaviour:**
1. System program allocates `Counter::INIT_SPACE` (+ discriminator)
   bytes at the counter's address, owned by this program.
2. Payer transfers lamports for rent-exempt minimum.
3. Counter's `count` is zeroed.

**Checks:** Anchor's `init` constraint ensures the account doesn't
already exist.

In the native and pinocchio variants, there is no `initialize_counter`
handler. The client (see `tests/`) uses the System program directly
to create, fund, and assign the account to the counter program in a
separate instruction.

### `increment`

**Who calls it:** anyone with lamports to pay the transaction fee.

**Signers:** none required beyond the fee-payer.

**Accounts in:**
- `counter` (writable) — the account to bump

**Behaviour:**
1. Deserialise the `u64` out of the counter's data.
2. Add 1 (checked add in anchor; unchecked in native; manual
   little-endian in pinocchio).
3. Serialise the new value back.
4. Log the new value.

**State changes:** `counter.count += 1`.

**Checks:**
- Anchor verifies the account type via the discriminator
  automatically.
- Native variant asserts `counter_account.is_writable`.
- Nothing checks *which* program owns the account in the native
  variant, which is a deliberate simplification — see "Safety"
  below.

**Token movements:** none.

## 5. Worked example

Starting from scratch using the Anchor variant:

```
1. Alice calls initialize_counter, passing a fresh keypair K.
   - K is created, owned by counter_anchor, count = 0.
   - Rent: ~0.00089 SOL paid by Alice.

2. Alice calls increment(counter = K).
   - count becomes 1.

3. Bob (a different wallet) calls increment(counter = K).
   - count becomes 2. This succeeds — anyone can bump it.

4. Alice calls initialize_counter again with the same K.
   - Fails: Anchor's `init` constraint sees the account already
     exists.
```

## 6. Safety and edge cases

This program is deliberately permissive. A few things worth noting:

- **Anyone can increment.** There's no check on the signer. For a
  real app you'd add an `authority` field on the account and
  constrain `authority.key() == signer.key()`.
- **Overflow.** Anchor uses `checked_add(1).unwrap()` — overflow
  panics. Native uses `counter.count += 1` — overflow would wrap in
  release mode, panic in debug. At one increment per second it would
  take ~584 billion years to overflow a u64, so this is academic.
- **Wrong account type (native variant).** The native handler
  deserialises with `Counter::try_from_slice`, which will decode any
  8-byte account data as a valid `Counter` regardless of owner. A
  production program must check `counter_account.owner == program_id`
  before trusting the data. Anchor does this automatically.
- **Double init.** In anchor, the `init` constraint prevents
  reinitialisation. The native variant doesn't have this instruction
  at all.
- **Unknown instruction bytes.** Native / pinocchio log "Error:
  unknown instruction" and return `Ok(())` — they don't fail the
  transaction on unknown discriminants. That's slightly surprising;
  a stricter implementation would return `ProgramError::InvalidInstructionData`.

## 7. Running the tests

Each subdirectory has its own test harness.

```bash
# Anchor (TypeScript tests)
cd anchor && anchor build && anchor test

# Native (solana-program + LiteSVM in Rust)
cd native && cargo build-sbf && cargo test

# Pinocchio (LiteSVM in Rust)
cd pinocchio && cargo build-sbf && cargo test

# MPL stack (shank + solita generated TS client)
cd mpl-stack && cargo build-sbf && pnpm install && pnpm test

# Quasar (embedded Rust tests via quasar-lang)
cd quasar && cargo test
```

LiteSVM is an in-process Solana runtime: no validator, no RPC, no
ledger. Anchor's test runner spins up `solana-test-validator` and
runs TS-based `mocha` tests against it.

## 8. Framework differences

| Variant | Account creation | Discriminator | Storage | Signer check on increment |
|---|---|---|---|---|
| `anchor/` | `init` constraint allocates + pays rent | Anchor 8-byte | `u64` with `InitSpace` | none (anyone) |
| `native/` | Client-side via System program | none | raw borsh 8 bytes | writable asserted |
| `pinocchio/` | Client-side via System program | none | raw little-endian 8 bytes | writable asserted |
| `mpl-stack/` | Client-side; Shank emits IDL and TS client via Solita | none | raw borsh 8 bytes | writable asserted |
| `quasar/` | `init` constraint on PDA (seeds `["counter", payer]`) | Quasar 1-byte (`= 1`) | `u64` | none |

Quasar's PDA seed `["counter", payer]` means each wallet has exactly
one counter; the other variants use fresh keypairs, so one wallet
can own many.

## 9. Extending the program

Some small improvements that each teach something:

- **Add an authority field.** Store `pub authority: Pubkey` on
  `Counter`; require the signer to match on increment. Moves from
  "shared bulletin board" to "per-user state".
- **Add a decrement instruction.** Introduces underflow handling.
- **Emit an event.** Add `emit!(CounterIncremented { new_value })` in
  anchor; clients can listen with `program.addEventListener`.
- **Make it a PDA in every variant.** Align on `["counter", payer]`
  (like quasar already does) and delete the keypair flow; teaches
  `find_program_address` and `invoke_signed`.
- **Close and refund.** Add a `close_counter` instruction that
  returns lamports to the payer. See `basics/close-account` for the
  pattern.
