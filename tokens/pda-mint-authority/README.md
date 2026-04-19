# PDA Mint Authority

A variant of `tokens/spl-token-minter` where the mint's address
*and* its mint/freeze authority *and* its metadata update authority
are all the same PDA — seeded by `["mint"]`. Neither a user wallet
nor a fresh keypair controls the mint; only the program, by
`invoke_signed`ing with the seed, can mint tokens.

Good reference for: "how do I gate mint authority behind program
logic?"

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

Two instructions, same shape as `spl-token-minter` but with a PDA
mint:

1. **`create_token(name, symbol, uri)`** — allocates a mint at the
   PDA address `["mint"]`, with `decimals = 9` and
   `mint_authority = freeze_authority = same PDA`. Then CPIs
   Metaplex's `create_metadata_accounts_v3`, with the PDA as the
   metadata update authority, signed via `with_signer(seeds)`.
2. **`mint_token(amount)`** — CPIs `token::mint_to` with the PDA
   as authority, again signed via `invoke_signed` with seeds.

Because the program is the only thing that can sign for the PDA,
minting is fully under program control. Any wallet can call
`mint_token` (no extra checks in this example), but *only* through
this program.

## 2. Glossary

**PDA (Program-Derived Address)**
: An address off the ed25519 curve, derived from seeds + program
id. Has no private key. The owning program signs for it by calling
`invoke_signed` with the seeds.

**`invoke_signed` / `with_signer(signer_seeds)`**
: Native Solana has `invoke_signed(&ix, &accounts,
&[&[seed_bytes...], &[bump]])`. Anchor wraps this as
`CpiContext::new(...).with_signer(signer_seeds)` — semantically
identical, just typed.

**Same PDA as both account and authority**
: The seeds `["mint"]` are used *twice*:
1. As the `seeds` on the mint's `#[account(init, seeds=["mint"], bump)]`
   — so the mint's address is a PDA.
2. As the `signer_seeds` passed to `mint_to` — so the program can
   sign for that same PDA as the mint authority.
This works because "being an authority" is just "being able to
produce the signature for this pubkey", and the program can
produce signatures for PDAs under its own program id.

**Canonical bump**
: The byte appended to the seeds that makes the hash land off-curve.
Anchor finds it at instantiation and makes it available as
`context.bumps.mint_account`.

## 3. Accounts and PDAs

### `create_token`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer, mut | SOL (rent + fee) | — | user |
| `mint_account` | PDA (Mint), init | `["mint"]` | `Mint { decimals: 9, mint_authority = self, freeze_authority = self }` | program (via seeds) |
| `metadata_account` | Metaplex PDA | `["metadata", metaplex_id, mint_account]` | `DataV2` | program (via seeds) |
| `token_program`, `token_metadata_program`, `system_program`, `rent` | programs/sysvars | — | — | — |

### `mint_token`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer, mut | SOL (pays ATA rent if created) | — | user |
| `mint_account` | PDA (Mint) | `["mint"]` | mint state | program (via seeds, as authority) |
| `associated_token_account` | ATA (mint, payer), init_if_needed | — | payer's balance | — |
| `token_program`, `associated_token_program`, `system_program` | programs | — | — | — |

## 4. Instruction lifecycle walkthrough

### `create_token(name, symbol, uri)`

1. Anchor's `init` allocates the mint PDA at `find_program_address(["mint"],
   program_id)`. Size = SPL `Mint::LEN`. Owned by the SPL Token
   program. SPL `initialize_mint` sets both authorities to the same
   PDA.
2. Build signer seeds = `&[&[b"mint", &[bump]]]`.
3. CPI Metaplex `create_metadata_accounts_v3` with
   `mint_authority = update_authority = mint_account` (the PDA),
   signed by the program via `with_signer(signer_seeds)`.

**Token movements:** none. Supply is zero.

**State changes:** mint PDA created; metadata PDA created.

**Checks:** payer signs; PDA seeds enforced by Anchor; Metaplex
verifies mint authority signature matches the PDA.

### `mint_token(amount)`

1. `init_if_needed` creates the ATA for (mint PDA, payer) if
   absent.
2. Build signer seeds = `&[&[b"mint", &[bump]]]`.
3. CPI `token::mint_to(mint, to = ata, authority = mint PDA,
   amount = amount × 10^9)` with the program signing as the PDA.

**Token movements:**

```
(no source) --[amount × 10^9 units]--> payer ATA (mint=PDA, owner=payer)
```

**State changes:** ATA created if needed; mint supply +=
`amount × 10^9`.

**Checks:** SPL Token program checks that the authority provided
(the PDA) matches `mint.mint_authority`. The program's signed
invoke proves authority.

## 5. Worked example

```
1. Program id P deployed. PDA = find_program_address(["mint"], P).
   Say PDA address = Mx.., bump = 254.

2. Alice calls create_token("PDA Coin", "PDA", "https://.../pdacoin.json").
   - Mint @ Mx.. created. Supply=0. mint_authority=Mx.., freeze=Mx...
   - Metadata PDA written. update_authority = Mx.. (the PDA).

3. Alice calls mint_token(amount = 100).
   - ATA for (Mx.., Alice) created.
   - mint_to(mint=Mx.., to=ata, authority=Mx.., amount=100 × 10^9)
     signed by the program with seeds ["mint", 254].
   - ata.amount = 100_000_000_000 = 100 tokens.

4. Alice tries to use an off-program tool (e.g. spl-token CLI)
   to mint more:
   spl-token mint Mx.. 50
   - Fails: nobody holds a private key for Mx... The CLI can't
     produce a signature. Only the program can sign.
```

## 6. Safety and edge cases

- **Only the program can mint.** Bus-factor: no lost key risk; the
  authority is fully embedded in the program code. Anyone who can
  upgrade the program, however, can redirect minting — so consider
  freezing the program upgrade authority for production.
- **Anyone can call `mint_token`.** This example doesn't gate the
  caller. Add a `require_keys_eq!(payer.key(), ADMIN)` or an
  authority PDA for real mints.
- **Single mint per program.** Seeds `["mint"]` have no variable
  component — there's exactly one mint per deployed instance. Add
  more seeds (e.g. a `seed: u64` arg) to support many.
- **Metadata update authority is the PDA.** Metadata updates go
  through the program. The example has `is_mutable = false` so this
  authority is moot; flip to `true` to make updates possible via a
  new program instruction.
- **`init_if_needed` pays ATA rent.** Payer eats the ~0.002 SOL for
  the first mint call per recipient.
- **Bump consistency.** Anchor looks up the canonical bump at
  runtime. If you cache a non-canonical bump client-side and pass
  it in, Anchor rejects. Storing the canonical bump on state is
  the idiomatic fix (as `basics/favorites` does).

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar && cargo test
```

Tests create the mint, mint to Alice's ATA, and verify her balance
plus that the mint authority is the PDA (not her wallet).

## 8. Extending the program

- **Gate `mint_token` with an admin.** Add `admin: Signer<'info>`
  and check `admin.key() == HARDCODED_ADMIN`. Only the admin may
  mint, even though the authority is the PDA.
- **Per-user mint PDA.** Change seeds to `["mint", user.key()]` so
  each user has their own token type.
- **Transfer authority away.** Add an instruction that CPIs
  `token::set_authority` to burn the mint authority (`None`), using
  the PDA as signer. Once done, supply is frozen forever.
- **Mint-capped supply.** Store a `max_supply` in a config PDA;
  reject `mint_token` if the resulting supply would exceed it.
- **NFT variant.** Set `decimals = 0` and add master edition
  creation — exactly `tokens/nft-minter` but with PDA authority.
