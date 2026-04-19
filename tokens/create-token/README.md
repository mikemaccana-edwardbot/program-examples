# Create Token

A program that creates a new SPL-Token Mint *and* its Metaplex
metadata account in one instruction. No tokens are actually minted
here — that's `tokens/spl-token-minter` — but after this runs the
token exists and has a name, symbol and icon URI.

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

One instruction: `create_token_mint(decimals, name, symbol, uri)`.

1. Allocates and initialises a new `Mint` account under the SPL
   Token program, with the caller as both mint and freeze authority.
2. CPIs into Metaplex's Token Metadata program to create a
   `MetadataV3` account for that mint, storing the name, symbol and
   URI (typically a pointer to an off-chain JSON with richer data +
   an image URL).

Supply starts at 0. A separate mint-to instruction (see
`tokens/spl-token-minter`) is needed to actually create tokens.

## 2. Glossary

**Mint (SPL Token)**
: A specific type of Solana account that defines an SPL token. It
stores: `is_initialized`, `supply`, `decimals`, `mint_authority`,
`freeze_authority`. One mint = one token type. There is no name,
symbol or icon in the mint itself; that's why metadata is separate.

**Decimals**
: The number of fractional digits a token has. A USDC-like token
uses 6 decimals, so `1_000_000 = 1 USDC`. NFTs use 0 decimals (each
unit is indivisible). Decimals are stored in the mint and cannot
change after initialisation.

**Mint authority**
: The pubkey allowed to call `mint_to` on this mint, creating new
supply. Can be `None` (mint disabled forever) or a pubkey.

**Freeze authority**
: The pubkey allowed to freeze individual token accounts (prevents
transfers out). Can be `None`.

**Metaplex Token Metadata program**
: Address `metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s`. Owns
`MetadataV3` PDAs, one per mint, seeded by
`["metadata", token_metadata_program_id, mint]`. Stores a `DataV2`
struct: `name, symbol, uri, seller_fee_basis_points, creators,
collection, uses`.

**URI (in metadata)**
: Usually a URL to an off-chain JSON like `{ "name": "...",
"symbol": "...", "image": "https://.../icon.png", "attributes":
[...] }`. Solana does not host images; clients fetch them from
this URI. IPFS or Arweave are common.

**`MetadataV3`**
: The third revision of Metaplex's metadata layout. Adds collection
info and usage tracking compared to V2.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer, mut | SOL (pays rent + fee); becomes mint authority + update authority | — | user |
| `mint_account` | keypair, init | — | `Mint { supply: 0, decimals, mint_authority: payer, freeze_authority: None, ... }` | mint keypair (at creation) |
| `metadata_account` | PDA, owned by Metaplex | `["metadata", metaplex_program_id, mint]` | `DataV2` | Metaplex program (via CPI) |
| `token_metadata_program` | program | — | — | — |
| `token_program` | program | — | — | — |
| `system_program` | program | — | — | — |
| `rent` | sysvar | — | — | — |

## 4. Instruction lifecycle walkthrough

### `create_token_mint(decimals, name, symbol, uri)`

**Who calls it:** anyone. The caller becomes mint authority.

**Signers:** `payer`, `mint_account` (new keypair).

**Step by step:**

1. Anchor's `init` constraint allocates `Mint::LEN` bytes at
   `mint_account` owned by the SPL Token program, and calls
   `token::initialize_mint(decimals, mint_authority = payer,
   freeze_authority = None)`.
2. Program logs intent.
3. CPI to Metaplex `create_metadata_accounts_v3`:
   - Creates a PDA at `["metadata", metaplex_id, mint]`.
   - Writes `DataV2 { name, symbol, uri, seller_fee_basis_points: 0,
     creators: None, collection: None, uses: None }`.
   - `is_mutable = false`, `update_authority_is_signer = true`
     (payer signs).

**Token movements:** none (no tokens minted yet). Payer pays rent
for both accounts (~0.0015 SOL for the mint + ~0.006 SOL for the
metadata).

**State changes:**
- New `Mint` account (supply 0).
- New `MetadataV3` PDA.

**Checks:**
- `payer` must sign.
- `mint_account` must sign (required by SPL Token
  `initialize_mint`).
- `metadata_account` PDA seeds must match Metaplex's expected
  derivation.

## 5. Worked example

```
1. Alice calls create_token_mint(
     decimals = 9,
     name     = "Joe Coin",
     symbol   = "JOE",
     uri      = "https://raw.githubusercontent.com/.../joe.json",
   ), signed by Alice and a fresh mint keypair K (address Mx..).

2. Resulting state:
     Mint @ Mx..
       supply = 0
       decimals = 9
       mint_authority   = Alice
       freeze_authority = None
     Metadata PDA @ find_program_address(
       ["metadata", metaplex_id, Mx..], metaplex_id)
       name = "Joe Coin"
       symbol = "JOE"
       uri = "https://.../joe.json"
       is_mutable = false
       update_authority = Alice

3. Alice now has a token type JOE. To hand out tokens she needs a
   second instruction (mint_to) — see tokens/spl-token-minter.
```

### Decimals, explained

```
With decimals = 9:
  raw_amount_in_mint = display_amount × 10^9
  1.5 JOE on the UI = 1_500_000_000 in the mint

With decimals = 0 (NFTs):
  1 NFT on the UI = 1 in the mint
```

## 6. Safety and edge cases

- **`is_mutable = false`.** The metadata here is *permanently*
  frozen — the name, symbol, and URI can never change. Flip to
  `true` at creation if you want to update later (Metaplex
  `update_metadata_accounts_v2` then needs the update authority to
  sign).
- **Collision on metadata PDA.** Metadata PDA seeds are fully
  determined by the mint. One metadata account per mint, forever.
  Creating a second fails with "already in use".
- **Mint keypair signing.** The mint *account keypair* must sign
  creation, exactly like any System::create_account target. You
  need to generate a fresh keypair client-side and include it as a
  signer. There's no PDA mint in this variant.
- **Metaplex rent.** Metaplex metadata accounts are larger (~679
  bytes), so ~0.006 SOL of rent. Payer bears both.
- **Update authority defaults to payer.** This example hard-codes
  `update_authority = payer`. For production you might want a
  multisig or a DAO PDA.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar && cargo test
```

The tests call `create_token_mint` with fixture data, then fetch
the mint and metadata PDAs and assert their fields.

## 8. Extending the program

- **Freeze authority.** Pass a `freeze_authority: Pubkey` and set
  it on the mint. Freezing ATAs lets you implement compliance
  features (KYC-gated tokens) or pause transfers.
- **Update authority = PDA.** Set the metadata update authority to
  a PDA owned by this program, so updates are gated by program
  logic rather than a private key. Teaches `invoke_signed` for
  Metaplex CPIs.
- **Mint supply in the same instruction.** Chain `mint_to` after
  metadata creation to pre-fund a treasury ATA in one call.
- **Collection support.** Pass `collection: Some(Collection { key,
  verified: false })` so the mint belongs to a Metaplex
  collection. Then a separate `verify_collection` call marks it
  verified.
- **Token-2022.** Swap `anchor_spl::token` for
  `anchor_spl::token_interface` and `anchor_spl::token_2022`, use
  metadata pointer extension to store metadata in the mint
  directly — no separate Metaplex account needed. See
  `tokens/token-extensions/metadata/`.
