# Repository Layout

A "carnival" program used purely as a demonstration of how to lay
out a larger Solana program. It has three instructions (`go_on_ride`,
`play_game`, `eat_food`) and three state types (`Ride`, `Game`,
`Food`), each split into its own module.

Nothing is written to chain. Every instruction just logs a verdict
("you can ride", "you can't", "out of tickets"). The *point* of the
example is the directory structure, not the behaviour.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Directory layout](#4-directory-layout)
5. [Instruction lifecycle walkthrough](#5-instruction-lifecycle-walkthrough)
6. [Worked example](#6-worked-example)
7. [Safety and edge cases](#7-safety-and-edge-cases)
8. [Running the tests](#8-running-the-tests)
9. [Extending the program](#9-extending-the-program)

## 1. What does this program do?

Three instructions, each takes a name + ticket count + target name,
looks up a hardcoded list (`get_rides()`, `get_games()`,
`get_foods()`), checks a few rules (enough tickets, tall enough for
rides, old enough for some foods), and logs the outcome. No
accounts, no state.

Real carnival programs don't log anything onchain — this is strictly
a shape exercise.

## 2. Glossary

**Processor**
: In the native variant, the file (`processor.rs`) that matches an
instruction discriminant byte to a handler function. Anchor
generates the equivalent of this at compile time from the `#[program]`
module.

**Instruction module**
: One Rust file per instruction, grouped under `src/instructions/`.
Each exposes a handler function plus (in native) an argument struct
+ accounts layout.

**State module**
: One file per domain object, grouped under `src/state/`. Holds
account layouts (when there's onchain state) and helpers. In this
example they're plain in-memory Rust structs because nothing hits
chain.

**Barrel file (`mod.rs`)**
: A `mod.rs` that re-exports every file in the folder, so the
outside world can `use crate::instructions::*`. Convention borrowed
from many Rust crates.

## 3. Accounts and PDAs

None. Every `#[derive(Accounts)]` struct is empty
(`CarnivalContext`, etc.). Every instruction takes arguments only.

## 4. Directory layout

```
src/
├── lib.rs               # entry point + #[program] module wiring
├── error.rs             # custom error types (empty here, ready for growth)
├── instructions/
│   ├── mod.rs           # re-exports
│   ├── eat_food.rs      # eat_food handler + data type
│   ├── get_on_ride.rs
│   └── play_game.rs
└── state/
    ├── mod.rs
    ├── food.rs          # Food struct + get_foods() hardcoded list
    ├── game.rs
    └── ride.rs
```

The **native** variant adds one extra file:

```
src/
├── processor.rs         # match on instruction discriminant → handler
└── ...                  # otherwise identical
```

Anchor generates the processor from the `#[program]` macro. Native
code has to hand-wire it.

### Why this layout?

- **One handler per file.** When `lib.rs` has ten `pub fn`s, its
  diff lights up for every feature. When handlers live in their own
  files, diffs stay local and code review stays sane.
- **`state/` for data types, `instructions/` for behaviour.** Mirrors
  MVC-style separation. Tests for handlers can mock state easily.
- **`error.rs` centralises custom errors.** Lets other modules
  `use crate::error::CarnivalError` without a big import list.
- **Mirrors the SPL.** The Solana Program Library itself uses this
  shape, so anyone familiar with SPL can navigate your repo.

## 5. Instruction lifecycle walkthrough

All three instructions follow the same pattern — a rule-check plus a
log — so a single walkthrough suffices.

### `go_on_ride(name, height, ticket_count, ride_name)`

**Who calls it:** anyone.

**Signers:** fee-payer only.

**Accounts in:** none.

**Behaviour:**
1. Look up `ride_name` in `ride::get_rides()`.
2. If not found → log "Sorry, no such ride." and return `Ok(())`.
3. If `ticket_count < ride.tickets` → log "need X tickets".
4. If `height < ride.min_height` → log "need to be X\" tall".
5. Else → log "You rode the <ride>!" (paraphrased).

Same idea for `play_game` (tickets + age rules) and `eat_food`
(tickets + age rule for some items).

**Token movements:** none.

**State changes:** none.

**Checks:** none enforced by the runtime. Every failure path just
logs and returns success — the transaction never errors.

## 6. Worked example

```
1. Alice calls go_on_ride(
     name = "Alice",
     height = 62,
     ticket_count = 5,
     ride_name = "Zero Gravity"
   )
   - Ride found: Zero Gravity (tickets=5, min_height=60).
   - tickets OK, height OK.
   - Logs: "You're about to ride the Zero Gravity!"

2. Bob calls go_on_ride(
     name = "Bob", height = 50, ticket_count = 10,
     ride_name = "Zero Gravity"
   )
   - tickets OK, height 50 < 60.
   - Logs: "Sorry Bob, you need to be 60" tall to ride the Zero Gravity!"
   - Still returns Ok. No transaction failure.
```

## 7. Safety and edge cases

- **No state means no security to think about.** Everything lives
  in instruction logs.
- **No `Err`.** All failure modes log and return `Ok`. That means a
  client can't tell from the transaction status whether the caller
  "actually" rode the ride. A production version would return
  `Err(CarnivalError::TooShort)` so the tx fails loudly and the
  error propagates.
- **Hardcoded lists.** `get_rides()` etc. return `Vec<Ride>` built
  from string literals. A real carnival would load these from a
  PDA.

## 8. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar && cargo test
```

The tests send a few instructions and inspect the program logs to
confirm the right branches were taken.

## 9. Extending the program

- **Turn log-only rejections into real errors.** Fill in `error.rs`
  with `#[error_code]` / `CarnivalError` variants and `return
  err!(CarnivalError::TooShort)` instead of a log + `Ok`.
- **Onchain tickets.** Put ticket counts in a per-user PDA; the
  carnival program deducts tickets on ride entry. Now the layout
  justifies the split — `state/ticket.rs` holds the real account
  type.
- **Catalogue PDAs.** Move `get_rides()` output into a PDA populated
  by an `initialize_park` instruction. The ride list becomes
  upgradeable.
- **Tests per instruction file.** Add a `#[cfg(test)] mod tests`
  inside each `instructions/*.rs`. Easier to run than the bundled
  `tests/` harness.
- **Feature flag per ride type.** Gate some rides behind Cargo
  features (`--features zero-gravity`). Shows how bigger SPL
  programs split optional behaviour.
