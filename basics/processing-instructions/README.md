# Processing Instructions (Custom Instruction Data)

A minimal program that deserialises two fields — a name and a height
— out of the instruction data, logs a welcome message, and decides
whether the caller is tall enough to ride a (imaginary) rollercoaster.

No accounts, no state, no CPIs. The whole point is to teach how
instruction data bytes get turned into typed Rust values on the
program side.

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

One instruction. The caller passes a struct:

```rust
struct InstructionData {
    name: String,
    height: u32,
}
```

serialised as borsh bytes. The program deserialises it and logs:

- `Welcome to the park, <name>!`
- If `height > 5`: `You are tall enough to ride this ride. Congratulations.`
- Else: `You are NOT tall enough to ride this ride. Sorry mate.`

That's everything. No onchain side-effects.

## 2. Glossary

**Instruction data**
: A byte slice attached to every instruction in a transaction. The
runtime hands it to the program's entrypoint as `&[u8]`. How those
bytes are interpreted is entirely up to the program — there's no
enforced schema.

**Borsh (Binary Object Representation Serializer for Hashing)**
: Solana's standard binary serialisation format. Deterministic (no
hash-map ordering ambiguity), compact, and easy to read/write in
Rust and TypeScript. Strings serialise as
`u32 length little-endian + UTF-8 bytes`; `u32`s are 4 bytes LE.

**BPF (Berkeley Packet Filter) / sBPF**
: The bytecode format Solana programs compile to. "BPF format" in
the older README comments is a slight misnomer — the *program* is
BPF; the *instruction data* is just bytes, by convention borsh. We
compile with `cargo build-sbf` (Solana BPF).

**Discriminator**
: When a program has multiple instructions, convention is to
reserve the first byte (native) or 8 bytes (Anchor) of instruction
data as an identifier. This program has only one instruction, so
Anchor still emits an 8-byte discriminator (the hash of
`"global:go_to_park"`) while the native variant just reads the
whole data as the struct directly.

**`#[derive(BorshSerialize, BorshDeserialize)]`**
: Derive macros that implement the serde traits for a Rust struct.
The native variant uses these to get `try_from_slice` for free.
Anchor derives borsh automatically for arguments of `#[program]`
functions.

## 3. Accounts and PDAs

None. `#[derive(Accounts)] struct Park {}` — zero accounts required.

## 4. Instruction lifecycle walkthrough

### `go_to_park(name: String, height: u32)` (anchor) / default (native)

**Who calls it:** anyone.

**Signers:** fee-payer only.

**Accounts in:** none.

**Instruction data layout:**

| offset | bytes | meaning |
|---|---|---|
| 0 | 8 | Anchor discriminator (Anchor variant only) |
| 0 / 8 | 4 | `name` length (u32 LE) |
| … | `len` | `name` UTF-8 bytes |
| … | 4 | `height` (u32 LE) |

**Behaviour:**
1. Deserialise the struct.
2. Log the welcome.
3. Branch on `height > 5`, log the verdict.
4. Return `Ok(())`.

**Token movements:** none.

**State changes:** none.

**Checks:**
- Anchor: matches the discriminator first; if mismatch, errors
  before calling the handler.
- Native: `try_from_slice` returns `Err` if the bytes aren't a valid
  `InstructionData` — that becomes the `ProgramError`.

## 5. Worked example

Client side (Anchor TS):

```ts
await program.methods.goToPark("Alice", 7).rpc();
```

Program logs:

```
Program <id> invoke [1]
Program log: Instruction: GoToPark
Program log: Welcome to the park, Alice!
Program log: You are tall enough to ride this ride. Congratulations.
Program <id> success
```

Client side (native, Rust):

```rust
let data = borsh::to_vec(&InstructionData {
    name: "Alice".to_string(),
    height: 7,
})?;
let ix = Instruction::new_with_bytes(program_id, &data, vec![]);
```

Bytes on the wire (no discriminator needed):

```
05 00 00 00  'A' 'l' 'i' 'c' 'e'  07 00 00 00
└──name len─┘ └───name bytes───┘ └──height──┘
```

## 6. Safety and edge cases

- **Invalid borsh.** A truncated or malformed payload makes
  `try_from_slice` return an error, which propagates as
  `ProgramError::BorshIoError`. The tx fails cleanly.
- **Extra trailing bytes.** Borsh's `try_from_slice` actually
  *rejects* trailing bytes in newer versions — the whole slice must
  be consumed. Older clients that pad their data may fail here.
- **Very long `name`.** The u32 length prefix caps at ~4 GB, but
  Solana's max transaction size (~1232 bytes) caps you far sooner.
  In practice `name` must fit in the transaction alongside the
  signatures and other headers; budget ~1 KB for the data.
- **Non-UTF-8 `name` bytes.** Borsh will error during deserialise.
- **Height of 0 / u32 overflow.** `height` is `u32`, so negative
  values aren't representable. There's no lower bound — 0 is "not
  tall enough", the expected answer.

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

The tests send a single instruction with a borsh'd struct and check
that the logs contain the expected welcome + ride verdict.

## 8. Framework differences

| Variant | Argument declaration | Deserialisation | Discriminator |
|---|---|---|---|
| `anchor/` | `fn go_to_park(ctx, name: String, height: u32)` — Anchor does it | automatic | 8-byte method hash |
| `native/` | `process_instruction(_, _, instruction_data: &[u8])` | `InstructionData::try_from_slice(instruction_data)` | none — raw struct |
| `pinocchio/` | explicit byte parsing | manual | none |
| `quasar/` | typed signature like Anchor | automatic | user-chosen byte |

Anchor is by far the shortest to write; native / pinocchio give you
full control over the wire format.

## 9. Extending the program

- **Add a second instruction.** E.g. `leave_park(reason: String)`.
  Teaches you how discriminators are actually needed once `>1`
  instruction exists.
- **Return data to the client.** Use `set_return_data(&[...])` to
  send a result back via `get_return_data` on the client. Handy for
  view-like calls.
- **Validate `name`.** Reject empty strings, non-ASCII characters, or
  strings above a max length.
- **Log with `sol_log_data`.** Emit structured binary logs instead of
  `msg!` strings — cheaper to parse off-chain.
- **Take a `Pubkey` arg.** A 32-byte fixed-size field. Compare against
  a hardcoded "VIP" pubkey and let that person ride regardless of
  height.
