# Shank & Solita (Car Rental Service)

A native-Solana car-rental program, built deliberately without
Anchor, to demonstrate how to use **Shank** (Rust macros + CLI from
Metaplex that generate an IDL from annotated native code) and
**Solita** (TypeScript codegen that turns that IDL into a typed
client).

The program itself is a toy: cars get added to a catalogue, users
book rentals, pick up, return. The point is the tooling — Anchor
gives you the IDL and TS client for free, but if you prefer to
write raw `solana_program` code, this shows you can still get an
Anchor-quality client experience.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [Shank in detail](#5-shank-in-detail)
6. [Solita in detail](#6-solita-in-detail)
7. [Worked example](#7-worked-example)
8. [Safety and edge cases](#8-safety-and-edge-cases)
9. [Running the tests](#9-running-the-tests)
10. [Extending the program](#10-extending-the-program)

## 1. What does this program do?

Four instructions:

1. **`AddCar(year, make, model)`** — creates a `Car` PDA
   (seeds `["car", program_id, make, model]`) holding the
   catalogue entry.
2. **`BookRental(args)`** — creates a `RentalOrder` PDA
   (seeds `["rental_order", program_id, car, payer]`) with status
   `Created`.
3. **`PickUpCar`** — advances the rental's status to `PickedUp`.
4. **`ReturnCar`** — advances to `Returned`.

It's a tiny state machine on a rental account. The interest lies
in how the accounts, PDAs and instructions are declared so Shank
can generate an IDL.

## 2. Glossary

**Native (non-Anchor) Solana**
: Writing a program directly against the `solana_program` crate,
without Anchor's attribute macros or state helpers. More verbose
but gives you full control — no hidden allocations, no runtime
constraint checks beyond what you write.

**IDL (Interface Definition Language)**
: A JSON description of a program's accounts, instructions, and
types. Think of it as the Solana equivalent of an OpenAPI spec or
a GraphQL schema. Clients use it to know how to encode /
decode data for the program. Anchor generates one from its
attribute macros; Shank generates one from its own macros on
native code.

**Shank**
: A Metaplex tool: `#[derive(ShankAccount)]`,
`#[derive(ShankInstruction)]`, `#[account(...)]` per-instruction
annotations. Plus the `shank` CLI (`cargo install shank-cli`) that
walks your crate and spits out `idl/<program>.json`.

**Solita**
: A Metaplex tool: a Node CLI (`yarn solita`) that reads an IDL
and writes typed TypeScript client code — instruction builders,
account decoders, PDA finders. Works on both Shank and Anchor
IDLs.

**`#[seeds(...)]`**
: Shank attribute on account structs that describes the account's
PDA seeds. Shank generates `Car::shank_pda(program_id, make,
model)` and `Car::shank_seeds_with_bump(...)` helpers from this.
Keeps seed definitions DRY — one source of truth in the state
module.

**Discriminant byte**
: Native programs disambiguate instructions by a byte (or more)
at the start of instruction data. Shank derives this from the
enum variant order (`AddCar = 0`, `BookRental = 1`, etc.) and
writes it into the IDL, so Solita can call the right thing.

## 3. Accounts and PDAs

### Account types

```rust
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, ShankAccount)]
#[seeds("car", program_id,
    make("The car's make", String),
    model("The car's model", String))]
pub struct Car {
    pub year: u16,
    pub make: String,
    pub model: String,
}

#[derive(..., ShankAccount)]
#[seeds("rental_order", program_id,
    car_public_key("The car's public key", Pubkey),
    payer_public_key("The payer's public key", Pubkey))]
pub struct RentalOrder {
    pub car: Pubkey,
    pub name: String,
    pub pick_up_date: String,
    pub return_date: String,
    pub price: u64,
    pub status: RentalOrderStatus,  // Created | PickedUp | Returned
}
```

### Per-instruction accounts

Each `CarRentalServiceInstruction` variant is annotated with its
expected accounts:

```rust
#[account(0, writable, name="car_account", desc="...")]
#[account(1, writable, name="payer", desc="Fee payer")]
#[account(2, name="system_program", desc="The System Program")]
AddCar(AddCarArgs),
```

Shank uses these annotations to emit instruction-level account
descriptors in the IDL.

| Instruction | Accounts (index, writable, name) |
|---|---|
| `AddCar` | 0: `car_account` (w), 1: `payer` (w), 2: `system_program` |
| `BookRental` | 0: `rental_account` (w), 1: `car_account`, 2: `payer` (w), 3: `system_program` |
| `PickUpCar` | 0: `rental_account` (w), 1: `car_account`, 2: `payer` (w) |
| `ReturnCar` | 0: `rental_account` (w), 1: `car_account`, 2: `payer` (w) |

## 4. Instruction lifecycle walkthrough

### `AddCar(year, make, model)`

1. Derive PDA from `["car", program_id, make, model]` via Shank's
   generated `Car::shank_pda`.
2. Assert the passed `car_account.key == pda`.
3. Compute borsh size of the `Car` struct, grab rent minimum.
4. `invoke_signed(system::create_account, ...)` signed by the PDA
   (seeds + bump from Shank's `Car::shank_seeds_with_bump`).
5. Serialise the `Car` data into the freshly allocated account.

**State changes:** new `Car` PDA exists with `{ year, make, model
}`.

### `BookRental(args)`

Same pattern: derive rental order PDA, `invoke_signed
system::create_account`, serialise a `RentalOrder` with status
`Created`.

### `PickUpCar` / `ReturnCar`

Read the rental account, match on status, overwrite with the new
status, reserialise.

## 5. Shank in detail

Install:

```bash
cargo install shank-cli
```

Shank needs `declare_id!(...)` somewhere in your crate — it uses
the program id in the IDL. Add whatever macros you need on your
types:

- `#[derive(ShankAccount)]` on account structs.
- `#[derive(ShankInstruction)]` on your instruction enum.
- `#[account(index, [writable], name, desc)]` on each enum variant
  to describe the accounts.
- `#[seeds(...)]` on any account whose address is a PDA.

Run `shank idl` from your crate and Shank writes
`idl/<program>.json`. That JSON is functionally identical to an
Anchor IDL.

## 6. Solita in detail

Install:

```bash
yarn add -D @metaplex-foundation/solita
```

Add a `.solitarc.js`:

```js
const path = require('path');
const programDir = path.join(__dirname, 'program');
const idlDir = path.join(programDir, 'idl');
const sdkDir = path.join(__dirname, 'tests', 'generated');
const binaryInstallDir = path.join(__dirname, '.crates');

module.exports = {
  idlGenerator: 'shank',
  programName: 'car_rental_service',
  idlDir,
  sdkDir,
  binaryInstallDir,
  programDir,
};
```

Run:

```bash
yarn solita
```

Solita writes TypeScript under `tests/generated/`:
- Instruction builders (`createAddCarInstruction(...)`).
- Account decoders (`Car.fromAccountInfo(...)`).
- PDA finders (`Car.findPDA(...)`).
- Types for each enum and struct.

Your tests then call these typed functions — the same ergonomics
you'd get from Anchor.

## 7. Worked example

```
1. Alice runs `shank idl` → idl/car_rental_service.json.
2. Alice runs `yarn solita` → tests/generated/*.ts.
3. Alice writes a test:
     const [carPda] = Car.findPDA("Honda", "Civic");
     const ix = createAddCarInstruction({ carAccount: carPda,
       payer: alice.publicKey, systemProgram: ... },
       { year: 2020, make: "Honda", model: "Civic" });
     await sendAndConfirmTransaction(tx.add(ix));
4. Onchain: Car PDA created with year=2020, make="Honda",
   model="Civic".
5. Alice runs BookRental, PickUpCar, ReturnCar in sequence.
   Each instruction's status advances.
```

## 8. Safety and edge cases

- **Manual PDA assertion.** `add_car` explicitly asserts the
  passed PDA matches derivation. Native code has no Anchor
  constraint to do this automatically, so you have to remember.
  Forgetting allows a caller to pass any writable account and have
  the program overwrite it.
- **No status-transition guard.** `PickUpCar` will happily
  overwrite a `Returned` status. The program doesn't verify the
  current status before transitioning. Production code should
  `match order.status` and return an error on illegal transitions.
- **Borsh string length.** `make` and `model` are `String` with no
  cap. Abuse by passing a multi-kilobyte string — eats rent and
  could push the transaction over its size limit.
- **Arithmetic.** No `u64` arithmetic here, but any amount math
  in native code should use `checked_*` operators and handle
  overflow explicitly.
- **Shank / Solita version drift.** Shank CLI and the
  `@metaplex-foundation/solita` npm package evolve independently.
  Pin versions; upgrading can break codegen silently.

## 9. Running the tests

```bash
cargo build-sbf
yarn install
yarn solita      # regenerate TS client from IDL
yarn test
```

The tests deploy the program to a local validator, call each
instruction via the Solita-generated client, and assert the PDAs
are populated as expected.

## 10. Extending the program

- **Transitions.** Guard `PickUpCar` to require `Created`, and
  `ReturnCar` to require `PickedUp`. Emit an error enum via a
  custom `ProgramError` implementation. Teaches you the native
  error-handling boilerplate Anchor hides.
- **Delete / close.** A `CancelRental` instruction that drains
  lamports from the rental PDA and zeroes its data — native
  equivalent of Anchor's `close = payer`.
- **Fees.** Charge a SOL fee from `payer` to an owner PDA on
  `BookRental`. Introduces System CPIs from within a native
  program.
- **Generate Rust client too.** Solita only does TS; you could
  write a Shank→Rust codegen that produces equivalent stubs for
  Rust integration tests.
- **Port to Anchor.** Same program, Anchor-style — half the
  boilerplate. Good side-by-side comparison for teaching why
  Anchor exists.

## References

- [Shank docs](https://docs.metaplex.com/developer-tools/shank)
- [Solita docs](https://docs.metaplex.com/developer-tools/solita)
