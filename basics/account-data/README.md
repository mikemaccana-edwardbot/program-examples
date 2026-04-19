# Account data

The smallest non-trivial Solana program: create one account, write
a struct into its data bytes, be done. No PDAs, no CPIs, no tokens —
just raw state.

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

One instruction: `create_address_info(name, house_number, street,
city)`. The caller supplies a fresh keypair for the new account; the
program initialises that account at the program's ownership, sizes it
to hold a serialised `AddressInfo` struct, and writes the four
fields in.

After the transaction:

- The new account exists.
- Its owner is this program.
- Its data bytes are an 8-byte Anchor discriminator followed by
  Borsh-serialised `{name, house_number, street, city}`.
- It holds enough lamports to be rent-exempt at that size.

## 2. Glossary

**Account**
: A Solana-addressed chunk of state: lamports, owner program, data
bytes. This program creates exactly one per call.

**Discriminator**
: The first 8 bytes of an Anchor account's data. Anchor writes it
at init (the first 8 bytes of `sha256("account:AddressInfo")`)
and checks it on every deserialisation so bytes from one struct
type can't be mistaken for another.

**Borsh**
: Binary Object Representation Serializer for Hashing. Default
serialisation format for Solana account data. Integers in
little-endian, strings as `u32 length + UTF-8 bytes`.

**Rent-exempt**
: An account whose lamport balance is ≥ the rent-exempt threshold
for its data size. The runtime will not delete it. Calculated from
`data_len` via `Rent::minimum_balance(data_len)`.

**InitSpace macro**
: Anchor's derive that computes the static size of a struct at
compile time, so you can write
`space = AddressInfo::DISCRIMINATOR.len() + AddressInfo::INIT_SPACE`
instead of hand-counting bytes. Strings need a `#[max_len(N)]`
cap so the size is bounded.

## 3. Accounts and PDAs

| Account | PDA? | Kind | Owner | Holds |
|---|---|---|---|---|
| `address_info` | no (fresh keypair supplied by caller) | state | program | `AddressInfo` struct |
| `payer` | no | wallet | System Program | Pays rent and transaction fee |
| `system_program` | no | program | — | Invoked by Anchor for the create+allocate+assign flow |

The `AddressInfo` struct, from `state/address_info.rs`:

```rust
pub struct AddressInfo {
    #[max_len(50)] pub name: String,
    pub house_number: u8,
    #[max_len(50)] pub street: String,
    #[max_len(50)] pub city: String,
}
```

Total size: 8 (discriminator) + 4+50 + 1 + 4+50 + 4+50 = 171 bytes.

## 4. Instruction lifecycle walkthrough

### `create_address_info`

**Who calls it:** anyone with a keypair for the new account and a
funded wallet.

**Signers:** `payer`, `address_info` (the new-account keypair — its
private key has to sign because creating an account requires proof
you own the target address).

**Accounts in:**

- `payer` (signer, mut — debited for rent)
- `address_info` (signer, mut, **init**)
- `system_program`

**Parameters:** `name: String`, `house_number: u8`, `street: String`,
`city: String`.

**Behaviour:**

1. Anchor runs three System Program CPIs under the hood via the
   `init` constraint:
   - `CreateAccount` (allocates lamports and space)
   - implicit `Allocate` + `Assign` (sets data length and owner
     to this program)
2. Writes the 8-byte discriminator.
3. Serialises the struct into the data buffer.

**Token movements:**

```
payer (lamports) --[rent-exempt reserve for 171 bytes]--> address_info
```

Plus the usual transaction fee from `payer` to whichever validator
processes the slot.

**State changes:** `address_info.data = [disc | name | house_no |
street | city]`.

**Checks:** Anchor's `init` constraint verifies the account was
uninitialised before the call.

## 5. Worked example

Off-chain, generate a fresh keypair `K`. Call:

```ts
await program.methods
  .createAddressInfo("Joe", 123, "Main Street", "Faketown")
  .accounts({
    payer: payerKeypair.publicKey,
    addressInfo: K.publicKey,
  })
  .signers([payerKeypair, K])
  .rpc();
```

After: `program.account.addressInfo.fetch(K.publicKey)` returns the
four fields.

## 6. Safety and edge cases

- **Strings exceeding `#[max_len(50)]`** will trip Anchor's
  serialisation check and the transaction reverts. This keeps the
  account size bounded.
- **Re-running `create_address_info` with the same `address_info`
  pubkey** fails — `init` requires the account to be uninitialised.
- **Forgetting to sign as `address_info`** fails the transaction:
  `CreateAccount` requires a signature from the new-account key.

## 7. Running the tests

```bash
cd anchor && anchor build && anchor test
cd native && cargo build-sbf && pnpm install && pnpm test
cd pinocchio && cargo build-sbf && cargo test
cd quasar && cargo test
```

## 8. Extending the program

- **Add an `update_address_info` instruction** that takes the same
  four fields, checks the caller is authorised (e.g. a PDA seeded by
  the previous signer), and overwrites the struct in place.
- **Make `address_info` a PDA** seeded by `["address_info", payer,
  counter]` so the caller no longer needs to supply a keypair.
  `basics/create-account` and `basics/pda-rent-payer` both do this.
- **Close the account** in a second instruction that returns the
  lamports to the payer.

## Framework differences

All four implementations do the same three things: create, size,
write. Differences:

- **Anchor** — `#[account]` + `InitSpace` auto-computes space; `init`
  constraint runs the CPIs.
- **Native** — manual `invoke_signed` for `CreateAccount` +
  `Allocate` + `Assign`, plus Borsh derive on the struct.
- **Pinocchio** — same three calls via `pinocchio_system` helpers;
  no heap allocation.
- **Quasar** — Anchor-like macros on the smaller quasar-lang runtime.
