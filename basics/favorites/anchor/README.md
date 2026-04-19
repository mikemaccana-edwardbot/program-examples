# Favorites

A per-user "profile" PDA that stores a favourite number, colour and
up to five hobbies. Each wallet can write exactly one `Favorites`
account (seeds: `["favorites", user]`) and only that wallet can
modify it.

Originally built for the Solana Professional Education course as a
first taste of per-user state via PDAs.

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

One instruction: `set_favorites(number, color, hobbies)`. It uses
`init_if_needed` on a PDA derived from the caller's pubkey, so the
first call creates the account and subsequent calls overwrite it.

The PDA seeds tie the account to the signer — so user Alice can
only write Alice's `Favorites`. There's no way for Bob to edit
Alice's row, because his derived PDA is at a different address.

## 2. Glossary

**PDA (Program-Derived Address)**
: An address computed from seeds + program id that sits off the
ed25519 curve (no private key). Only the owning program can "sign"
for a PDA, via `invoke_signed`. PDAs give you deterministic
per-user / per-thing account addresses.

**Canonical bump**
: Program-derived addresses are found by brute-forcing an extra byte
(`bump`) until `sha256(seeds || bump || program_id)` is off-curve.
The canonical bump is the *largest* such byte that works (runtime
starts at 255 and decrements). Storing it lets later instructions
re-derive without the expensive search. Here it's cached in
`favorites.bump`.

**`init_if_needed`**
: Anchor constraint that creates the account if it doesn't exist, or
does nothing if it does. Convenient for "upsert" flows. Requires
the `init-if-needed` feature in the Anchor.toml / Cargo.toml.

**`#[derive(InitSpace)]`**
: Anchor macro that computes the byte size of the struct at compile
time, using `#[max_len(...)]` for variable-length fields (strings
and vecs). The PDA is allocated
`Favorites::DISCRIMINATOR.len() + Favorites::INIT_SPACE` bytes.

**Signer**
: A wallet whose private key signed the transaction. Only the signer
is recognised in the `user` account slot, and the PDA derivation
uses `user.key()` — so only that user can access their PDA.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `user` | signer, mut | — | SOL (pays rent, pays fee) | user |
| `favorites` | PDA, program-owned | `["favorites", user]` | `number: u64`, `color: String(≤50)`, `hobbies: Vec<String>(≤5 of ≤50)`, `bump: u8` | program (via seeds) |
| `system_program` | program | — | — | — |

PDA size breakdown:

```
8 (disc) + 8 (number) + 4+50 (color) + 4 + 5*(4+50) (hobbies) + 1 (bump)
= 8 + 8 + 54 + 4 + 270 + 1
= 345 bytes
```

Rent for 345 bytes ≈ 0.0024 SOL on mainnet.

## 4. Instruction lifecycle walkthrough

### `set_favorites(number, color, hobbies)`

**Who calls it:** any wallet. Acts as both create and update.

**Signers:** `user`.

**Accounts in:**
- `user` (signer, mut)
- `favorites` (PDA, `init_if_needed`, `seeds = ["favorites", user]`)
- `system_program`

**Behaviour:**
1. PDA is derived from `["favorites", user.key()]`.
2. If it doesn't exist: System program allocates 345 bytes owned by
   this program, payer = user, rent deducted from user.
3. Handler calls `favorites.set_inner(Favorites { number, color,
   hobbies, bump })`. `set_inner` replaces the whole struct, so
   updates are full overwrites.
4. The canonical bump is cached in `favorites.bump`.

**Token movements:** on first call, user pays rent to the PDA. On
subsequent calls, no movement.

**State changes:** `favorites` fully overwritten. First call also
allocates it.

**Checks enforced by Anchor:**
- `user` must be the signer.
- PDA address must match derivation from `["favorites", user]` +
  canonical bump — Bob can't pass Alice's PDA address.
- `color.len() <= 50` at deserialisation time (Anchor enforces
  `max_len` when reading; writing past the cap would overflow the
  account's data buffer and be rejected).
- `hobbies.len() <= 5`, each entry `<= 50`.

## 5. Worked example

```
1. Alice (pubkey Ax..) calls set_favorites(
     number = 7,
     color  = "chartreuse",
     hobbies = ["climbing", "rust", "tea"]
   )
   - PDA derived: pda_A = find_program_address(["favorites", Ax..], program_id)
   - Account doesn't exist → allocated, 345 bytes, owned by program.
   - Rent ~0.0024 SOL deducted from Alice.
   - Data: { number: 7, color: "chartreuse", hobbies: [...], bump: 254 (say) }

2. Alice calls set_favorites(number=8, color="teal", hobbies=["sleep"]).
   - Same PDA. init_if_needed sees it exists — skips creation.
   - set_inner overwrites to { number: 8, color: "teal", hobbies: ["sleep"], bump: 254 }.

3. Bob calls set_favorites(number=42, ...).
   - Different PDA: pda_B = find_program_address(["favorites", Bx..], program_id).
   - His own fresh account, separate from Alice's.

4. Bob passes pda_A (Alice's) and calls set_favorites.
   - Anchor derives pda_expected = find_program_address(["favorites", Bx..], program_id)
     which != pda_A. Constraint fails.
   - Error: ConstraintSeeds / AccountNotFound.
```

## 6. Safety and edge cases

- **PDA seeds bind the signer.** Bob can never write Alice's row —
  the seed derivation includes the signer's pubkey. This is the
  whole point.
- **`init_if_needed` is `init` + a check.** It's safe here because
  the writer is always the same user. If you reuse this pattern
  where different signers might hit the same PDA, consider that
  `init_if_needed` lets the first caller *create* the account — and
  whoever creates it pays rent.
- **Oversized inputs.** Anchor deserialisation enforces `#[max_len]`.
  Passing a 51-char colour string will error at instruction borsh
  decode time.
- **Hobby count.** Exactly 5 hobbies allowed (Vec of max 5 × 50
  bytes). Passing 6 fails the same way.
- **Repeated writes.** Every call fully overwrites the state. There's
  no field-level update; if you want one, add explicit
  `update_color` / `update_hobbies` instructions.
- **Close flow.** Not implemented. You cannot currently reclaim the
  rent once written. See `basics/close-account` for the pattern.

## 7. Running the tests

```bash
anchor build
anchor test
```

The tests spin up `solana-test-validator`, send one `set_favorites`
invocation, fetch the PDA and assert the fields match.

## 8. Extending the program

- **Add `get_favorites`.** A view-only instruction that logs the PDA's
  contents. (Mentioned in a comment in `lib.rs` but not implemented.)
- **Add `close_favorites`.** Reclaim rent to the user. Use Anchor's
  `close = user` constraint.
- **Partial updates.** Replace the single `set_favorites` with
  `set_number(u64)`, `set_color(String)`, `set_hobbies(Vec<String>)`
  so callers can update one field without re-sending everything.
- **Shared profiles.** Drop the signer-in-seeds constraint so a group
  can edit one profile, gated by an `authority` field.
- **Validation.** Enforce non-empty colour, ASCII-only strings, etc.
