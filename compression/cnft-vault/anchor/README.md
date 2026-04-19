# cNFT Vault

A PDA-owned vault for compressed NFTs. The PDA `["cNFT-vault"]` is
the `leaf_owner` of cNFTs sent to it. The program exposes two
withdraw instructions that CPI into Bubblegum's `Transfer`,
`invoke_signed`ing as the vault PDA to hand the cNFT to a new owner.

Two variants:
- `withdraw_cnft` — one cNFT out.
- `withdraw_two_cnfts` — two cNFTs out in one instruction. Not two
  instructions — exactly one, because each transfer needs its own
  proof and bundling them avoids a second transaction round-trip.

Deposits happen *outside* the program: the user transfers the cNFT
to the vault PDA using any standard Bubblegum transfer.

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

- **Receive cNFTs (external):** any Bubblegum holder can transfer
  their cNFT to the vault PDA. No instruction in this program
  needed — it's just a regular transfer where the recipient
  happens to be the PDA.
- **`withdraw_cnft(root, data_hash, creator_hash, nonce, index)`:**
  CPIs `Bubblegum::Transfer` from `leaf_owner = vault PDA` to
  `new_leaf_owner = <caller-specified>`. Signed by the program via
  `invoke_signed` with seeds `["cNFT-vault"]`.
- **`withdraw_two_cnfts(...)`:** same idea, twice. Accepts two sets
  of proof parameters and does two CPIs.

Anybody can withdraw any cNFT in the vault. There's no
authorisation — the point is the PDA-as-owner + proof-passing
mechanics, not access control.

## 2. Glossary

See `compression/cnft-burn/anchor/README.md` for Merkle tree,
proof, Bubblegum, spl-account-compression, `data_hash`,
`creator_hash`, `nonce`, `index` definitions.

**Vault PDA**
: Seeds `["cNFT-vault"]`. Single vault, program-wide. Holds no
data (System-owned in spirit — cNFTs don't live in accounts, they
live in the Merkle tree; the PDA just appears as a Pubkey in
leaves).

**`invoke_signed`**
: The runtime call that lets the program sign for the PDA during
the CPI. Here signer seeds are `[b"cNFT-vault", &[bump]]`.

**`AccountMeta::new_readonly(..., false)`**
: Used for proof siblings and most cNFT accounts — they're not
written to and not signers.

## 3. Accounts and PDAs

### `withdraw_cnft`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `tree_authority` | PDA (owned by Bubblegum) | `[merkle_tree]` | tree config | — |
| `leaf_owner` | PDA (vault) | `["cNFT-vault"]` | — (PDA with no account) | program (via seeds) |
| `new_leaf_owner` | any pubkey | — | — | — |
| `merkle_tree` | compression account, mut | — | tree state | — |
| `log_wrapper` | spl-noop | — | — | — |
| `compression_program` | program | — | — | — |
| `bubblegum_program` | program | — | — | — |
| `system_program` | program | — | — | — |
| `remaining_accounts` | N proof siblings | — | — | — |

### `withdraw_two_cnfts`

Same layout, but with two sets of `(root, data_hash, creator_hash,
nonce, index)` arguments and two merkle trees + proofs passed via
the struct and `remaining_accounts`. See `instructions/withdraw_two_cnfts.rs`.

## 4. Instruction lifecycle walkthrough

### `withdraw_cnft(root, data_hash, creator_hash, nonce, index)`

**Who calls it:** anyone.

**Behaviour:**
1. Build metas for each proof sibling from `remaining_accounts`
   (`AccountMeta::new_readonly(acc.key(), false)`).
2. Build a Bubblegum `Transfer` instruction:
   - `tree_authority` (ro)
   - `leaf_owner` = vault PDA (ro, **signer** from PDA perspective)
   - `leaf_delegate` = vault PDA (ro)
   - `new_leaf_owner` (ro)
   - `merkle_tree` (writable)
   - `log_wrapper`, `compression_program`, `system_program` (ro)
   - + proof siblings (all ro)
3. Data = `TRANSFER_DISCRIMINATOR ++ borsh(TransferArgs)`.
4. `invoke_signed(&instruction, &account_infos, &[&[b"cNFT-vault",
   &[bump]]])`. The PDA's "signature" is derived from the seeds.

**State changes:** the cNFT's leaf now records
`new_leaf_owner` instead of the vault PDA. Merkle root updates.

**Checks enforced downstream:**
- Bubblegum verifies the proof.
- Bubblegum verifies the current leaf owner is the vault PDA.
- Bubblegum verifies someone has signed as that owner — in this
  case the program, via its PDA seeds.

### `withdraw_two_cnfts`

Two independent `Transfer` CPIs. Each has its own proof accounts
slice from `remaining_accounts`; the two slices are concatenated
and the instruction expects the client to pass them in the right
order and length.

## 5. Worked example

```
1. Alice mints cNFT C, owner = Alice.
2. Alice calls Bubblegum::Transfer directly (not via this program)
   to transfer C to new_leaf_owner = vault PDA. C is now
   "deposited" in the vault.
3. Bob wants C. He fetches its current proof (now showing vault PDA
   as leaf owner) from DAS.
4. Bob calls withdraw_cnft on this program with:
     leaf_owner = vault PDA
     new_leaf_owner = Bob
     + proof accounts
   The program signs as the PDA via seeds ["cNFT-vault"].
   Bubblegum verifies proof, sees owner = vault PDA, sees signature
   from program = valid authority, transfers to Bob.
5. C now belongs to Bob.
```

Note how step 4 has no check on *who* is calling. Bob, Alice,
anyone could have withdrawn C. The README says so openly — it's a
reference implementation.

## 6. Safety and edge cases

- **No access control.** Anyone can drain the vault. For a real
  program, gate withdrawals on some condition: a per-cNFT lock, an
  admin signature, a game rule, etc.
- **`bubblegum_program` unchecked.** The `UncheckedAccount` allows
  a malicious caller to pass a fake program. The `invoke_signed`
  will target *whatever* they pass. Easy fix: require
  `bubblegum_program.key() == MPL_BUBBLEGUM_ID`.
- **Proof staleness.** Like all cNFT operations, the `root` must
  match the current tree root (or recent enough to still be in the
  changelog buffer). Tree writes from other users can invalidate
  your in-flight transaction.
- **Delegate hardcoded to owner.** `leaf_delegate = leaf_owner =
  vault PDA`. If a delegate was set externally on a deposited cNFT,
  this transfer would fail. Real programs would track and pass a
  delegate correctly.
- **No event log.** Users watching the vault have to rely on
  Bubblegum's emitted logs via `log_wrapper`. This program adds no
  event of its own, so indexing is a bit indirect.
- **Two-cNFT compute cost.** Each cNFT transfer with a depth-14
  proof + canopy spends non-trivial compute. Two in one
  instruction bumps the compute used significantly — you may need
  to raise the compute budget with a `ComputeBudgetProgram`
  instruction in the same transaction.

## 7. Running the tests

```bash
cd anchor
anchor build
anchor test
```

Tests set up a tree, mint cNFTs, transfer them into the vault,
then call the withdraw instructions. Requires an RPC with DAS
support (`getAssetProof`) — the test scripts point at Helius by
default.

Additional scripts in `tests/scripts/` demonstrate the same flow
with address lookup tables (`withdrawWithLookup.ts`) which is
essential when you pass many proof accounts — Solana's per-tx
account limit is 64, and a depth-20 tree + the base accounts eats
~28.

## 8. Extending the program

- **Access control.** Require the caller to equal the `depositor`
  stored in a per-cNFT state account. Deposit becomes a real
  instruction that records who sent the cNFT in.
- **Lockup / vesting.** Add a `release_at: i64` field; reject
  withdraw if `Clock::unix_timestamp < release_at`.
- **Validate `bubblegum_program`.** Reject any program id other
  than the real Bubblegum.
- **Bulk withdraw.** Generalise `withdraw_two_cnfts` to N cNFTs,
  parsing `remaining_accounts` per a count argument. Bump compute
  budget accordingly.
- **Fee on withdraw.** Send a small SOL fee from the withdrawer
  to the vault admin before CPI'ing the cNFT transfer.

## Reference

- [Helius DAS API](https://docs.helius.dev/compression-and-das-api/das-api)
- [Solandy's video walkthrough](https://youtu.be/qzr-q_E7H0M) of
  this program's original Bubblegum integration.
