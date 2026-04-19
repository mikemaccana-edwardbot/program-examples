# NFT Minter

One instruction: creates a brand-new mint (0 decimals), mints exactly
one token into the payer's ATA, writes Metaplex metadata, then
creates a `MasterEditionV3` account which transfers the mint and
freeze authorities to itself — locking supply at 1 forever.

The result is a canonical Metaplex NFT owned by the payer.

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

`mint_nft(name, symbol, uri)` does four things in one transaction:

1. Allocates and initialises a new `Mint` with `decimals = 0`,
   `mint_authority = payer`, `freeze_authority = payer`.
2. Creates an associated token account for the payer (if not
   already present) and CPIs `mint_to(amount = 1)`.
3. CPIs `create_metadata_accounts_v3` on Metaplex to attach name,
   symbol and URI to the mint.
4. CPIs `create_master_edition_v3` on Metaplex. This transfers the
   mint and freeze authorities from `payer` to the master edition
   PDA, preventing any future minting.

After this instruction: exactly 1 unit of a 0-decimal token exists,
owned by the payer; neither the payer nor anyone else can ever mint
more of it.

## 2. Glossary

**NFT (Non-Fungible Token)**
: An SPL token with `decimals = 0` and `supply = 1` where minting
is permanently disabled. "Non-fungible" means each token is
distinct (identified by its mint address), as opposed to fungible
tokens where every unit is interchangeable.

**Metaplex Token Metadata program**
: Address `metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s`. Defines
the `MetadataV3` and `MasterEditionV3` account types used to mark
mints as NFTs.

**Master Edition**
: A PDA associated with a mint (seeds `["metadata", metaplex_id,
mint, "edition"]`) that records the NFT's "edition" status. When
created via `create_master_edition_v3`:
- The mint's `mint_authority` and `freeze_authority` are
  transferred from whoever owned them to the master edition PDA.
- Future calls to `mint_to` fail — only the PDA is authorised, and
  the Metaplex program only signs for it under specific
  edition-printing instructions (used for "prints" of an NFT).

**Max supply**
: Passed as `None` here, meaning the NFT is a unique master
edition — you cannot print prints of it. Set `Some(N)` to allow
up to N prints.

**Fungible vs NFT differences (short version)**
: Fungible token: decimals > 0, supply grows as mint authority
  mints more, authorities often stay live.
: NFT: decimals = 0, supply = 1 (one mint = one NFT), authorities
  transferred to master edition PDA so supply is locked.

**`seller_fee_basis_points`**
: Royalty rate in basis points (1% = 100). Stored on metadata so
marketplaces can honour royalties. Set to 0 here.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer, mut | SOL (pays rent + fee); becomes mint + freeze authority briefly | — | user |
| `mint_account` | keypair, init | — | `Mint { decimals: 0, supply: 1 (after), mint_authority: master edition }` | mint keypair |
| `associated_token_account` | ATA (mint, payer), init_if_needed | — | holds the single NFT | — |
| `metadata_account` | Metaplex PDA | `["metadata", metaplex_id, mint]` | name / symbol / uri | — |
| `edition_account` | Metaplex PDA | `["metadata", metaplex_id, mint, "edition"]` | master edition struct | — |
| `token_program`, `token_metadata_program`, `associated_token_program`, `system_program`, `rent` | programs/sysvars | — | — | — |

## 4. Instruction lifecycle walkthrough

### `mint_nft(name, symbol, uri)`

**Who calls it:** anyone. The caller becomes the NFT's first owner.

**Signers:** `payer`, `mint_account` (new keypair).

**Step by step:**

1. Anchor `init` on `mint_account`: allocate, initialise as
   `decimals = 0`, mint+freeze authorities = payer.
2. Anchor `init_if_needed` on `associated_token_account`: allocate
   ATA for (mint, payer).
3. CPI `token::mint_to(mint, to = ata, authority = payer, amount =
   1)`. The mint's supply becomes 1.
4. CPI Metaplex `create_metadata_accounts_v3`: creates metadata PDA
   with `DataV2 { name, symbol, uri, seller_fee_basis_points: 0,
   creators: None, collection: None, uses: None }`,
   `is_mutable = false`.
5. CPI Metaplex `create_master_edition_v3`:
   - Creates edition PDA.
   - Transfers mint + freeze authorities from `payer` to the
     edition PDA.
   - `max_supply = None` (unique NFT — no prints).

**Token movements:**

```
(no source) --[1 unit]--> payer ATA (mint=M, owner=payer)
```

After step 5, the mint authority is no longer `payer`. The runtime
enforces this: further `mint_to` from any signer fails.

**State changes:** new mint, new ATA, new metadata PDA, new master
edition PDA. Mint's `mint_authority` and `freeze_authority` are
reassigned.

**Checks:**
- Payer signs; mint keypair signs.
- Metaplex PDA seeds validated.
- Metaplex enforces that the caller IS the current mint authority
  when creating the master edition — since we just initialised the
  mint with `payer` as authority, this works.

## 5. Worked example

```
1. Alice calls mint_nft(
     name   = "Solana Sunset #1",
     symbol = "SUN",
     uri    = "https://.../sunset1.json"
   )
   signed by Alice + fresh keypair M.

2. After the instruction:
     Mint @ M:
       decimals = 0
       supply = 1
       mint_authority   = <edition PDA>   (WAS Alice, transferred)
       freeze_authority = <edition PDA>   (WAS Alice, transferred)

     Alice's ATA for M:
       amount = 1

     Metadata PDA:
       name = "Solana Sunset #1", symbol = "SUN", uri = "..."

     Master Edition PDA:
       supply = 0 (prints; there are none)
       max_supply = None

3. Alice tries to call spl-token-minter::mint_token on M.
   - Fails: mint_authority is now the edition PDA, which she can't
     sign for.

4. Alice transfers the NFT to Bob via an SPL-Token transfer.
   Alice ATA -> Bob ATA, amount = 1.
```

## 6. Safety and edge cases

- **Irreversible authority transfer.** Once the master edition
  exists, there is no way to mint more of this NFT. No instruction
  in Metaplex reverses this.
- **`is_mutable = false`.** Metadata is permanently frozen — the
  name, symbol, and URI can never change. Flip to `true` if you
  want updatable metadata.
- **Mint keypair signing.** You must include the new mint's
  keypair as a signer. Typically the client generates a fresh
  keypair for every NFT.
- **ATA rent paid by payer.** ~0.002 SOL. Since the payer is also
  the initial owner, they pay for their own ATA.
- **`max_supply = None` vs `Some(0)`.** `None` means "master edition
  only, no prints allowed". `Some(0)` has similar effect on the
  Metaplex side. `Some(N)` would let `mint_new_edition_from_master_edition_via_token`
  create up to N numbered prints (not shown here).
- **Collection not set.** `collection: None` — this NFT isn't
  marked as part of any Metaplex collection. Add one to group NFTs
  together and get the blue verification tick in marketplaces.
- **Zero royalties.** `seller_fee_basis_points: 0`. Common for
  test fixtures; real projects set 100–1000 bps.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar && cargo test
```

Tests mint an NFT, assert the ATA balance is 1, and check the
metadata PDA contents.

## 8. Extending the program

- **Collection support.** Pass a `collection_mint: Option<Pubkey>`
  on the instruction; set it in the metadata and call Metaplex's
  `verify_collection_v1` afterwards.
- **Transfer instruction.** Wrap `token::transfer` so the program
  can mediate NFT transfers (e.g. checking a royalty payment).
- **Burn NFT.** Metaplex has `burn_nft` which also closes the
  metadata / edition PDAs. Useful for game items or compressed
  assets.
- **PDA mint authority before the edition is created.** See
  `tokens/pda-mint-authority` for that variant.
- **Move to Token-2022 metadata extension.** Remove Metaplex
  entirely and use the Token-2022 metadata extension to store
  `name / symbol / uri` *in the mint account itself*. Simpler; see
  `tokens/token-extensions/nft-meta-data-pointer/`.
