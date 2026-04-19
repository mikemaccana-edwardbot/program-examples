# cNFT Utils (cutils)

Two reference instructions for working with compressed NFTs from
inside an Anchor program:

1. **`mint(params)`** — CPIs Metaplex Bubblegum's
   `mint_to_collection_v1` to mint a cNFT into an existing
   collection tree, with your program logic wrapped around it.
2. **`verify(params)`** — CPIs spl-account-compression's
   `verify_leaf` to cryptographically prove the caller is the
   current `leaf_owner` of a specific cNFT, without transferring
   it. Useful for "is this person holding asset X?" gating.

Both instructions demonstrate Anchor's `#[access_control]` +
validate/actuate split, a pattern for separating permission checks
from the CPI side-effects.

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

### `mint`
Takes a URI, builds a Bubblegum `MintToCollectionV1` instruction,
and invokes it. The point of routing this through your own program
(rather than the client calling Bubblegum directly) is that you
can:
- Initialise your own per-mint state account alongside the cNFT.
- Enforce per-program rules (rate limits, access control, pricing)
  before the mint happens.
- Compose cNFT minting with other onchain effects atomically.

### `verify`
Computes the expected leaf hash from `(asset_id, leaf_owner,
leaf_delegate, nonce, data_hash, creator_hash)` and CPIs
`verify_leaf` on spl-account-compression. Bubblegum doesn't have a
built-in "prove-without-transferring" instruction — this is it.

## 2. Glossary

See `compression/cnft-burn/anchor/README.md` for: cNFT, Bubblegum,
spl-account-compression, Merkle proof, tree authority, `data_hash`,
`creator_hash`, `nonce`, `index`, canopy, log wrapper.

**Collection (Metaplex)**
: A set-id for grouping NFTs (compressed or regular). Enforced via
a "collection NFT" (a normal Metaplex NFT) whose mint is recorded
on each member's metadata. Marketplaces group and display by
collection.

**`mint_to_collection_v1`**
: The Bubblegum instruction to mint a cNFT *and* attribute it to a
Metaplex collection atomically. Requires:
- The tree delegate to sign (authorises minting into the tree).
- The collection authority to sign (authorises membership).
- The collection mint, metadata, and edition account.

**`verify_leaf`**
: An instruction on spl-account-compression. Takes the Merkle
`root`, a computed `leaf` hash, the `index`, and a proof
(siblings in `remaining_accounts`). Succeeds silently if the proof
is valid; errors otherwise.

**Asset id**
: A deterministic identifier for a cNFT, computed as
`sha256("asset", merkle_tree, nonce_le)[0..32]`. Unique per
(tree, nonce).

**Leaf schema v1 hash**
: The cNFT's leaf value in the tree. Computed as
`sha256(version | asset_id | owner | delegate | nonce | data_hash
| creator_hash)`. Must match the on-chain leaf at `index` for
proof verification to succeed.

**`#[access_control(...)]` (Anchor)**
: Macro attribute that runs a function before the handler's body.
If the function returns `Err`, the instruction aborts before any
state change. A neat way to centralise permission checks.

**`validate` + `actuate`**
: Convention borrowed from larger Solana projects. `validate`
returns `Ok` iff preconditions are satisfied; `actuate` does the
work. `#[access_control(ctx.accounts.validate(...))]` wires
`validate` as the gate for `actuate`.

## 3. Accounts and PDAs

### `mint` (Bubblegum `MintToCollectionV1`)

A long account list, because this is a collection-verified mint:

| name | kind | who signs |
|---|---|---|
| `payer` | signer, mut | user (pays rent) |
| `tree_authority` | PDA (owned by Bubblegum), mut | — |
| `leaf_owner` | any pubkey | — |
| `leaf_delegate` | any pubkey | — |
| `merkle_tree` | compression account, mut | — |
| `tree_delegate` | signer | tree admin |
| `collection_authority` | signer | collection admin |
| `collection_authority_record_pda` | PDA or Bubblegum id | — |
| `collection_mint` | Metaplex collection NFT mint | — |
| `collection_metadata` | Metaplex metadata account | — |
| `edition_account` | Metaplex master edition | — |
| `bubblegum_signer` | Bubblegum PDA used as signer | — |
| `log_wrapper`, `compression_program`, `token_metadata_program`, `bubblegum_program`, `system_program` | programs | — |

### `verify` (spl-account-compression `verify_leaf`)

| name | kind | who signs |
|---|---|---|
| `leaf_owner` | signer | owner of the cNFT |
| `leaf_delegate` | any pubkey | — |
| `merkle_tree` | compression account | — |
| `compression_program` | program | — |
| `remaining_accounts` | proof siblings | — |

## 4. Instruction lifecycle walkthrough

### `mint(params: MintParams { uri: String })`

**Who:** anyone who can get both the tree delegate and the
collection authority to sign.

**Behaviour:**
1. `access_control → validate` (currently a no-op; returns `Ok`).
   This is where a real program would add rules — rate limits,
   paywall checks, tier gates.
2. Build Bubblegum `MintToCollectionV1` args:
   - `MetadataArgs { name, symbol, uri, seller_fee_basis_points,
     creators, token_standard = NonFungible, collection: Some,
     ... }`.
3. Build the instruction: hardcoded discriminator +
   borsh(args) + the account metas in Bubblegum's expected order.
4. `invoke(...)`. The outer signers (`payer`, `tree_delegate`,
   `collection_authority`) carry into the CPI.

**Token movements:** none (the "token" here is a cNFT, which is
just a leaf in the tree — not an SPL account).

**State changes:** a new leaf is written at the next index in the
tree; the root hash updates; Metaplex records the NFT as a member
of the collection.

**Checks:**
- Anchor: `tree_delegate` and `collection_authority` must sign.
- Bubblegum: checks the collection authority record PDA is either
  a real authority or the Bubblegum program id (delegation fallback).
- Bubblegum: checks the tree delegate matches the tree's config.

### `verify(params: VerifyParams)`

**Who:** the current `leaf_owner` of a specific cNFT.

**Behaviour:**
1. `validate` returns `Ok` (no extra rules).
2. Compute `asset_id = get_asset_id(merkle_tree, nonce)`.
3. Compute `leaf_hash = leaf_schema_v1_hash(asset_id,
   leaf_owner, leaf_delegate, nonce, data_hash, creator_hash)`.
4. Build the `verify_leaf` CPI: discriminator
   (`sha256("global:verify_leaf")[..8]`) + `root + leaf_hash +
   index_le`, accounts = `merkle_tree + proof_siblings`.
5. `invoke(ix, account_infos)`. The compression program hashes
   `leaf_hash` up with the provided siblings at `index` and checks
   that the top equals `root`. Errors if it doesn't.

**Token movements:** none.

**State changes:** none (read-only CPI).

**Checks:**
- `leaf_owner` must sign — this binds the caller's identity to the
  proof. Anyone knowing the proof can *construct* a verify call,
  but only the owner's signature proves they *are* the owner.

## 5. Worked example

```
Mint:
  Alice has a collection C already (normal Metaplex NFT).
  Alice's tree T is set up; she is the tree delegate.
  Alice signs as both tree_delegate and collection_authority.
  Alice calls mint(params = { uri: "https://.../alice1.json" })
  with leaf_owner = Bob.
  -> new cNFT minted, owner = Bob, collection = C.

Verify (later):
  Some other program wants to confirm Bob owns the cNFT.
  Bob fetches (root, data_hash, creator_hash, nonce, index, proof)
  from DAS.
  Bob signs a tx calling verify(params = { root, data_hash,
  creator_hash, nonce, index }) with remaining_accounts = proof.
  -> success iff Bob is still the owner and the proof is valid.
  -> fails otherwise.
```

Composed usage: a game contract might require `cutils::verify` to
succeed before applying a power-up to the cNFT's holder. Because
the verify CPI fails hard on mismatch, the wrapping instruction
aborts and no state changes.

## 6. Safety and edge cases

- **`bubblegum_program` and friends unchecked.** All
  `UncheckedAccount`s mean the program trusts what the caller
  passes. Easy fix: check each key against the hardcoded constants
  (the mpl-bubblegum and token-metadata IDs) at the top of the
  handler. Pasted in for brevity in the example.
- **`validate` is a no-op.** The hook is there to be filled in per
  application. Don't ship this unmodified for anything important.
- **Collection authority signing.** Requiring
  `collection_authority` as a signer tightly couples this
  instruction to the collection owner. Production apps often use
  a collection *authority record PDA* so a separate admin can
  mint without the collection owner's key. Bubblegum supports
  both; the `collection_authority_record_pda` slot is where that
  lives.
- **Proof stale/invalid on verify.** `verify_leaf` errors on
  mismatch. Callers should tolerate this — tree writes from
  concurrent users can invalidate a just-fetched proof. Retry
  with a fresh fetch.
- **Anchor / mpl-bubblegum version pin.** The original code
  worked around a dependency conflict (Anchor 1.0's
  solana-program 3.x vs mpl-bubblegum's 2.x types) by hand-building
  the CPI instructions instead of using the SDK's CPI wrappers.
  Once Metaplex ships a matching version, switch to the wrappers
  for less boilerplate.

## 7. Running the tests

```bash
cd anchor
# 1) if you don't have a tree/collection yet
ts-node tests/setup.ts
# 2) run the tests
anchor build
anchor test
```

Tests go through the mint + verify path against a local or devnet
deploy. Needs an RPC with DAS (Helius is common).

## 8. Extending the program

- **Fill in `validate`.** Examples:
  - `require_keys_eq!(ctx.accounts.payer.key(), WHITELIST_ADMIN);`
  - Limit total mints per tree in a per-tree config PDA.
  - Charge a SOL fee up front (CPI `system::transfer` in `validate`
    — actually better placed in `actuate`, since `validate` is
    read-only by convention).
- **Program-owned per-cNFT state.** Add an `init` for an
  `AssetMeta { asset_id, owner, xp, level, ... }` PDA seeded by
  `asset_id` inside `mint::actuate`. Game stats become onchain.
- **Use mpl-bubblegum-cpi SDK.** Swap hand-built instructions for
  the Metaplex CPI helpers once the dependency conflict is
  resolved.
- **Gate by verify.** Add a second instruction that calls verify
  internally, then does something (e.g. grants an SPL token
  reward). Pattern for "proof-of-ownership-required" flows.
