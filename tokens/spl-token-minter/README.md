# SPL Token Minter

Two instructions: `create_token` (makes a new mint + Metaplex
metadata) and `mint_token` (mints `amount` tokens to the recipient's
ATA, creating the ATA if needed).

This is the natural next step after `tokens/create-token`. Where
create-token only makes the mint, this one also hands out supply.

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

- **`create_token(name, symbol, uri)`** — allocates a new SPL
  `Mint` (9 decimals, payer as both mint and freeze authority) and
  creates its Metaplex `MetadataV3` PDA with the given metadata.
- **`mint_token(amount)`** — CPIs `token::mint_to` to credit
  `amount × 10^9` units into the recipient's ATA. If the ATA
  doesn't exist, `init_if_needed` creates it first.

The caller of `mint_token` must be the mint authority — in this
example that's the same wallet that created the mint.

## 2. Glossary

**SPL Token program**
: Solana's canonical fungible token program, at address
`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`. Owns every mint and
every token account on the network (unless you're on Token-2022).

**Mint**
: Defines a token type. Holds `decimals`, `supply`, `mint_authority`,
`freeze_authority`. See `tokens/create-token` for more.

**Token Account**
: An account that holds a *balance* of one specific token type for
one owner. Essentially a tuple (mint, owner, amount).

**Associated Token Account (ATA)**
: The canonical, deterministically-addressed token account for a
given (mint, owner) pair. Address:
`find_program_address(["[owner]", TOKEN_PROGRAM_ID, mint],
ATA_PROGRAM_ID)`. There's at most one ATA per (mint, owner) — any
wallet can look up any other wallet's balance by deriving it
without needing to know an address out of band.

**`init_if_needed`**
: Anchor constraint that creates an account if absent, skips if
present. Used here so the first `mint_token` call also creates the
recipient's ATA.

**Mint authority**
: Pubkey allowed to call `mint_to` on this mint. Stored in the
mint. Must sign every mint.

**Decimals-adjusted amount**
: When a caller passes `amount = 100`, the program multiplies by
`10^decimals` (here `10^9`) before calling `mint_to`. So "100 JOE"
on the UI is `100_000_000_000` in the mint's `supply` counter.

## 3. Accounts and PDAs

### `create_token`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `payer` | signer, mut | SOL (rent + fee); becomes mint & freeze authority | — | user |
| `mint_account` | keypair, init | — | mint data (supply 0, decimals 9) | mint keypair |
| `metadata_account` | PDA (owned by Metaplex) | `["metadata", metaplex_id, mint]` | `DataV2` (name, symbol, uri) | Metaplex via CPI |
| `token_program`, `token_metadata_program`, `system_program`, `rent` | programs/sysvars | — | — | — |

### `mint_token`

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `mint_authority` | signer, mut | SOL (pays rent for ATA if created) | — | mint authority |
| `recipient` | system account | — | — | — |
| `mint_account` | existing mint, mut | supply increases here | — | — |
| `associated_token_account` | ATA (mint, recipient), `init_if_needed` | ATA derivation | recipient's balance | — |
| `token_program`, `associated_token_program`, `system_program` | programs | — | — | — |

## 4. Instruction lifecycle walkthrough

### `create_token(name, symbol, uri)`

1. Allocate and initialise the mint: 9 decimals, mint authority =
   payer, freeze authority = payer.
2. CPI `metaplex::create_metadata_accounts_v3` to create the
   metadata PDA with `DataV2 { name, symbol, uri, ... }`.

**Token movements:** none — supply is zero.

**State changes:** new mint, new metadata PDA.

**Checks:** payer signs; mint_account keypair signs; metadata PDA
seeds validated by Anchor.

### `mint_token(amount)`

1. If the recipient's ATA doesn't exist, Anchor creates it
   (payer = mint_authority).
2. CPI `token::mint_to(mint, to = ata, authority = mint_authority,
   amount = amount × 10^decimals)`.

**Token movements:**

```
(no source)  --[amount × 10^9 units]-->  recipient ATA (mint, owner=recipient)
```

Strictly speaking it's not a transfer — it's mint-from-nothing.
`mint_account.supply += amount × 10^9`.

**State changes:** ATA created if needed; ATA balance increases;
mint supply increases.

**Checks:**
- `mint_authority` must sign and must match
  `mint_account.mint_authority`. The SPL Token program enforces
  this.
- ATA's (mint, owner) must match `mint_account` and `recipient`.
- If mint authority is `None`, the mint is frozen and `mint_to`
  fails.

## 5. Worked example

```
1. Alice calls create_token("Joe Coin", "JOE", "https://.../joe.json")
   signed by Alice + fresh keypair K.
   - Mint @ K created with decimals=9, authority=Alice.
   - Metadata PDA created.

2. Alice calls mint_token(amount = 100), with
     mint_authority = Alice
     recipient = Bob
     mint_account = K
   - ATA for (K, Bob) is created (rent paid by Alice).
   - mint_to(mint=K, to=ata, authority=Alice, amount=100 × 10^9).
   - ata.amount = 100_000_000_000 = 100 JOE on the UI.
   - K.supply = 100_000_000_000.

3. Alice calls mint_token(amount = 50, recipient = Bob) again.
   - ATA already exists; init_if_needed skips.
   - ata.amount becomes 150_000_000_000 = 150 JOE.
```

## 6. Safety and edge cases

- **Only mint authority can call `mint_token`.** The SPL Token
  program rejects otherwise. A dropped / burned authority
  (`set_authority` to `None`) permanently disables further minting.
- **`init_if_needed` pays rent.** Every first-time recipient costs
  the `mint_authority` the ATA rent (~0.002 SOL). A production
  minter would often require the recipient to pre-create their own
  ATA to avoid this.
- **Overflow on `amount × 10^decimals`.** The multiplication uses
  `u64`. With `decimals = 9`, the largest safe input is ≈ 18.4
  billion tokens per mint call. Higher values wrap and panic.
- **Supply vs balance.** `mint.supply` is the sum of all ATA
  balances for this mint. They must stay in sync (the SPL Token
  program enforces this via its instructions — you can't hand-edit
  balances).
- **Authority pubkey laid bare.** The mint authority is written
  plaintext in the mint. For privacy-sensitive minting, use a
  multisig authority or a PDA.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar && cargo test
```

The tests create a mint + metadata, then mint a round amount and
assert the recipient's ATA balance matches.

## 8. Extending the program

- **Cap the supply.** Store a max in a separate `MintConfig` PDA;
  reject `mint_token` if `supply + amount > max`.
- **PDA mint authority.** Replace `mint_authority = payer` with a
  PDA; sign mint operations via `invoke_signed`. See
  `tokens/pda-mint-authority` for exactly that.
- **Revoke minting.** Add an instruction that CPIs
  `token::set_authority` to set the mint authority to `None`.
  Locks supply forever — turns the token into a fixed-supply asset.
- **Token-2022 with a metadata-pointer extension.** Use
  Token-2022's built-in metadata extension instead of Metaplex.
  See `tokens/token-extensions/metadata/` and `nft-meta-data-pointer/`.
- **Burn.** Add a `burn_tokens(amount)` instruction CPIng into
  `token::burn` from an ATA. Supply decreases.
