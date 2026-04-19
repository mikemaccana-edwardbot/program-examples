# cNFT Burn

One instruction: burn a compressed NFT via a CPI into the Metaplex
Bubblegum program. The leaf's owner signs; the proof nodes come in
as `remaining_accounts`.

The point is the proof-passing pattern. Compressed NFT instructions
are unusual: a single instruction might need 10–30 extra accounts
for the Merkle proof, which Anchor's typed accounts can't express
upfront. The program uses Anchor's `remaining_accounts` escape hatch
for this.

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

`burn_cnft(root, data_hash, creator_hash, nonce, index)` forwards
the inputs to Bubblegum's `Burn` instruction via CPI:

```
this program (signed by leaf_owner)
 └── CPI → mpl-bubblegum::Burn
      └── CPI → spl-account-compression::replace_leaf (sets leaf to zero)
```

Effect: the cNFT's leaf in the Merkle tree is zeroed out. The
proof is supplied by the caller in `remaining_accounts`; Bubblegum
verifies it against the on-chain root before accepting the burn.

## 2. Glossary

**Compressed NFT (cNFT)**
: An NFT stored not as a full SPL mint + metadata + edition, but
as a single leaf in a Merkle tree owned by the Bubblegum program.
You store only the tree's root on chain; individual NFT data lives
off-chain at indexers (Helius, DAS). Much cheaper — millions of
cNFTs can exist for less than one regular NFT's rent.

**Merkle tree**
: A binary tree where each non-leaf node is the hash of its two
children. The "root" hash at the top commits to the entire tree.
To prove a leaf is present you supply the sibling hashes along the
path from leaf to root — that's the "proof".

**Merkle proof**
: An array of sibling hashes (each 32 bytes, one per tree depth).
For a tree of depth 14 you pass 14 sibling accounts (actually
passed as Pubkeys — the compression program treats them as raw
32-byte blobs).

**Bubblegum program**
: Metaplex's cNFT program. Address
`BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY`. Defines cNFT
creation, transfer, delegate, burn, verify instructions.

**SPL Account Compression program**
: Underlying primitive at
`cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK`. Manages the Merkle
tree state (root, changelog, canopy). Bubblegum CPIs into it.

**Tree authority (PDA)**
: Seeds `[merkle_tree.key()]`, owned by Bubblegum. Stores per-tree
config (collection id, tree delegate, etc.). Every Bubblegum
instruction requires it.

**Log wrapper (spl-noop)**
: A dummy program (`noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV`) that
Bubblegum emits logs through. Used so indexers can see state-change
events via log introspection.

**`data_hash` / `creator_hash`**
: 32-byte commitments to the NFT's on-chain data and creator array.
Bubblegum hashes these along with other fields into the leaf value;
they must match the indexer's reported values for the proof to
verify.

**`nonce` / `index`**
: The `nonce` identifies the leaf within the Merkle tree's
creation order; `index` is the canonical leaf index. They're the
same number in most cases.

**`remaining_accounts`**
: Anchor's "unknown-in-advance accounts" slot. Anything after the
declared `#[derive(Accounts)]` fields comes in here. Used for the
Merkle proof because its length depends on tree depth.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `leaf_owner` | signer, mut | — | SOL (pays fee) | owner of the cNFT |
| `tree_authority` | PDA (owned by Bubblegum) | `[merkle_tree]` | tree config | — |
| `merkle_tree` | account (owned by spl-account-compression), mut | — | Merkle tree state | — |
| `log_wrapper` | program (spl-noop) | — | — | — |
| `compression_program` | program (spl-account-compression) | — | — | — |
| `bubblegum_program` | program (mpl-bubblegum) | — | — | — |
| `system_program` | program | — | — | — |
| `remaining_accounts` | N Pubkeys (proof nodes) | — | proof hashes | — |

## 4. Instruction lifecycle walkthrough

### `burn_cnft(root, data_hash, creator_hash, nonce, index)`

**Who calls it:** the cNFT's leaf owner.

**Behaviour:**
1. Build instruction data: Bubblegum's `Burn` discriminator (the
   hardcoded `[116, 110, 29, 56, 107, 219, 42, 93]`) + borsh of
   `BurnArgs { root, data_hash, creator_hash, nonce, index }`.
2. Build account metas in Bubblegum's expected order:
   - `tree_authority` (ro)
   - `leaf_owner` (ro, **signer**)
   - `leaf_delegate = leaf_owner` (ro, not signer — this program
     assumes no delegate)
   - `merkle_tree` (writable)
   - `log_wrapper` (ro)
   - `compression_program` (ro)
   - `system_program` (ro)
   - then each of `remaining_accounts` as ro non-signer (the proof)
3. `invoke(&instruction, &account_infos)`. The outer signer
   (`leaf_owner`) is inherited by the CPI.

**State changes:** Bubblegum → spl-account-compression zeroes the
leaf at `index`. Root hash updates. Changelog ring buffer advances.

**Checks enforced downstream:**
- Bubblegum hashes the proof up and compares to the on-chain root.
  Mismatch → CPI fails.
- Bubblegum compares `leaf_owner` against the leaf's stored owner
  hash. Mismatch → `Unauthorized`.

## 5. Worked example

```
1. Alice owns cNFT with nonce=42, index=42 in tree T.
2. Alice's indexer (Helius, DAS) tells her:
     root         = 0xAB...
     data_hash    = 0x12...
     creator_hash = 0x34...
     proof        = [sibling_0, sibling_1, ..., sibling_13]
3. Alice builds a transaction:
     - instruction target: cnft_burn
     - accounts:
         leaf_owner         = Alice (signer)
         tree_authority     = find_program_address([T], Bubblegum)
         merkle_tree        = T
         log_wrapper        = spl-noop
         compression_program = spl-ac
         bubblegum_program   = mpl-bubblegum
         system_program      = 111...
         + 14 remaining accounts (each proof sibling)
     - data: root ++ data_hash ++ creator_hash ++ nonce_le ++ index_le
4. cnft_burn CPIs Bubblegum::Burn. Bubblegum verifies the proof.
5. Leaf at index 42 is zeroed. root hash updates.
6. Alice's cNFT no longer exists.
```

## 6. Safety and edge cases

- **Delegate not supported.** The program hardcodes
  `leaf_delegate = leaf_owner`. If a delegate was set on the cNFT,
  only the delegate can burn — and this instruction wouldn't pass
  their pubkey. A production version would add an optional
  `leaf_delegate` account.
- **Proof staleness.** The Merkle root moves every time a cNFT in
  the same tree is modified. `root` must match the *current* root,
  or one recently enough that it's still in the changelog buffer
  (default 64 entries). Stale roots fail with `LeafHashMismatch` or
  similar. Clients typically fetch-then-send quickly.
- **Index mismatch.** If `nonce` or `index` don't match the leaf,
  the proof won't hash to `root`.
- **Anyone can pass wrong `bubblegum_program`.** `bubblegum_program`
  is declared `UncheckedAccount` — the program trusts whatever the
  caller passes as the Bubblegum program. A malicious caller could
  point it at a fake program. The runtime catches this because the
  *real* Bubblegum is what the account metas expect to invoke; a
  fake one would have a different program id. Still, checking
  `bubblegum_program.key() == MPL_BUBBLEGUM_ID` at the top would be
  a cheap safety belt.
- **Burn is irreversible.** There is no unburn. Once a cNFT's leaf
  is zeroed, it's gone; the leaf can later be reused for a new
  mint, but the original is permanent history.

## 7. Running the tests

```bash
cd anchor
anchor build
anchor test
```

The tests deploy the program, create a tree + collection, mint a
cNFT, fetch its proof via a DAS/Helius RPC, and then call
`burn_cnft`. You'll need to point the test config at an RPC that
supports the DAS API (`getAsset`, `getAssetProof`) — the default is
Helius.

## 8. Extending the program

- **Add the delegate account.** Take `leaf_delegate: Pubkey` (an
  optional signer) so delegate-initiated burns work.
- **Verify `bubblegum_program.key()` at entry.** Protects against a
  malicious caller passing a lookalike program.
- **Batch burn.** Loop over multiple leaves in a single instruction,
  each with its own proof. Gets compute-heavy fast.
- **Permissioned burn.** Check `leaf_owner` against an allowlist PDA
  so only authorised addresses can burn (e.g. a game that burns
  items on use).
- **Switch to `mpl-bubblegum-cpi`.** Metaplex publishes a crate
  with typed instruction builders so you don't have to hand-craft
  the discriminator and account metas.
