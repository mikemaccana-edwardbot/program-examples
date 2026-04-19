# NFT with Metadata Pointer (Token-2022)

An NFT whose `name`, `symbol`, `uri`, and **arbitrary key/value
pairs** (`level`, `wood`) live *inside the mint account itself* via
Token-2022's `MetadataPointer` extension. No separate Metaplex
metadata account.

Wrapped around this is a small lumberjack game: each wallet has a
`PlayerData` PDA tracking energy (auto-refilled with time) and
wood. Calling `chop_tree` spends 1 energy and gains 1 wood. The
NFT represents the player and its `level` metadata field can be
updated as they progress. Session keys (Magic Block) let mobile /
Unity clients auto-approve transactions without constant wallet
prompts.

Comes with a TypeScript Next.js client (`app/`) and a Unity client
(`unity/`) that both talk to the same Anchor program.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [The metadata-pointer extension in detail](#5-the-metadata-pointer-extension-in-detail)
6. [The lazy-energy pattern](#6-the-lazy-energy-pattern)
7. [Session keys](#7-session-keys)
8. [Worked example](#8-worked-example)
9. [Safety and edge cases](#9-safety-and-edge-cases)
10. [Running the example](#10-running-the-example)
11. [Extending the program](#11-extending-the-program)

## 1. What does this program do?

Three instructions:

1. **`init_player(level_seed)`** — creates a `PlayerData` PDA
   (seeds `["player", signer]`) with `energy = MAX_ENERGY`,
   `last_login = now`, `wood = 0`, `authority = signer`. Also
   creates a shared `GameData` PDA if absent.
2. **`chop_tree(level_seed, counter)`** — lazy-updates energy,
   spends 1, grants 1 wood. Callable by the player's main wallet
   OR a session key (via the `#[session_auth_or]` macro from the
   Magic Block / Gum session-key crate).
3. **`mint_nft()`** — creates a Token-2022 NFT with a metadata
   pointer pointing back at the mint itself; initialises the
   embedded metadata with `name = "Beaver"`, `symbol = "BVA"`,
   `uri = <arweave URL>`; adds a custom `level = "1"` field;
   creates an ATA for the player; mints 1 unit; revokes mint
   authority.

The NFT *represents* the character; the `PlayerData` PDA stores
the mutable state; metadata updates (e.g. "level up") happen by
writing new custom fields onto the mint.

## 2. Glossary

**Token-2022 MetadataPointer extension**
: A Token-2022 mint extension that stores a *pointer* (another
Pubkey) indicating where the metadata for this mint lives. When
`metadata_address == mint_address`, the metadata is stored in the
mint's *own* TLV data — no separate account needed. Saves one
account and keeps everything colocated.

**Embedded metadata (TLV)**
: After the core mint fields and the `MetadataPointer` TLV entry,
the `TokenMetadata` extension adds a variable-size TLV entry with
`name: String`, `symbol: String`, `uri: String`, plus
`additional_metadata: Vec<(String, String)>`. Those strings are
why this mint is allocated with extra space (`meta_data_space =
250` in the code).

**`update_field`**
: Token-2022 `TokenMetadata` instruction that sets a custom
key/value pair. Called by the metadata update authority. Level-ups,
XP, equipped item IDs — all live here as strings.

**`PlayerData` PDA**
: Per-wallet game state. Seeds `["player", signer]`. Stores
`authority, energy, last_login, wood`.

**`GameData` PDA**
: Global counter (e.g. total trees chopped). Seeds `["gameData"]`.
Created on first `init_player`.

**`nft_authority` PDA**
: Seeds `["nft_authority"]`. Used as mint and metadata update
authority for every NFT this program creates. Because the PDA is
controlled by the program, the program can later update metadata
(level-ups) without the player signing.

**Session key (Magic Block / Gum)**
: A short-lived keypair the client generates, funded with some SOL
and authorised to sign a specific program's instructions for ~23
hours. After that the token expires and the SOL returns. Lets
mobile/Unity games feel like web2 apps — no wallet prompt per
action.

**`#[session_auth_or(condition, error)]`**
: Anchor attribute macro from the session-keys crate. Before the
handler runs, checks either:
1. The session-key token is valid and whitelists this instruction,
   OR
2. The fallback `condition` is true (here: `player.authority ==
   signer`).
Neither passes → error `GameErrorCode::WrongAuthority`.

**Lazy update**
: Instead of a cron job adding energy every 60 seconds, the
program computes *how much* energy should have regenerated
between `last_login` and now whenever an instruction runs. Saves
rent and CPU — the player's view is always correct but state
only updates on demand.

**Arweave URI**
: The `uri` field points at an Arweave-hosted JSON descriptor (and
image). Arweave is pay-once-store-forever storage — popular for
NFT metadata because it's irreversible and cheap.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `player` | PDA | `["player", signer]` | `PlayerData { authority, energy, last_login, wood }` (size 1000 bytes) | program |
| `game_data` | PDA | `["gameData"]` | `GameData { ... }` (global state, size 1000) | program |
| `signer` | signer, mut | SOL | — | user (main wallet or session) |
| `mint` | keypair, init (Token-2022 mint + metadata) | — | mint + MetadataPointer ext + TokenMetadata ext (~270 bytes + fields) | mint keypair (creation), nft_authority (metadata updates) |
| `nft_authority` | PDA | `["nft_authority"]` | — | program |
| `token_account` | ATA (mint, signer) | — | holds 1 unit | — |
| `token_program` | Token-2022 | — | — | — |
| `associated_token_program`, `system_program` | programs | — | — | — |

## 4. Instruction lifecycle walkthrough

### `init_player(level_seed)`

1. `init` on `PlayerData` PDA: 1000 bytes, payer = signer.
2. `init_if_needed` on `GameData` PDA.
3. Set `energy = MAX_ENERGY`, `last_login = now`, `wood = 0`,
   `authority = signer.key()`.

### `chop_tree(level_seed, counter)`

1. `#[session_auth_or]` checks authorisation (session token or
   main authority).
2. Call `update_energy()` on the player:
   ```rust
   while time_passed >= TIME_TO_REFILL_ENERGY && energy < MAX_ENERGY {
     energy += 1;
     time_passed -= TIME_TO_REFILL_ENERGY;
     time_spent += TIME_TO_REFILL_ENERGY;
   }
   ```
3. If `energy == 0` → `Err(NotEnoughEnergy)`.
4. Else `wood += 1`, `energy -= 1`, log.

### `mint_nft()`

Long sequence — this is the meat of the extension lesson:

1. Compute space = `ExtensionType::try_calculate_account_len::<Mint>(
   &[MetadataPointer]) + 250` (the 250 is padding for the embedded
   token metadata TLV).
2. `system::create_account(from=signer, to=mint, lamports=<rent>,
   space, owner=token_program)`.
3. `system::assign(mint, &token_2022::ID)`.
4. `metadata_pointer::instruction::initialize(mint,
   authority=Some(nft_authority), metadata_address=Some(mint))`
   via `invoke`. Must happen *before* `initialize_mint`.
5. `initialize_mint2(mint, decimals=0, authority=nft_authority,
   freeze_authority=None)` via Anchor helper.
6. Build signer seeds `[b"nft_authority", bump]`.
7. `spl_token_metadata_interface::initialize(mint, update_authority
   = nft_authority, mint_authority = nft_authority, name =
   "Beaver", symbol = "BVA", uri = <arweave>)` via
   `invoke_signed` (PDA signs as metadata update authority).
8. `update_field(mint, key="level", value="1")` via
   `invoke_signed`.
9. Create ATA for (mint, signer).
10. `mint_to(mint, ata, authority = nft_authority, amount = 1)`
    via `invoke_signed`.
11. `set_authority(mint, AuthorityType::MintTokens, new_authority
    = None)` via `invoke_signed` — disables further minting.

**Token movements:**

```
(no source) --[1 unit]--> player ATA (mint, owner=player)
```

Mint authority goes from `nft_authority` to `None` at the end.
Supply locks at 1. Metadata is permanently owned by `nft_authority`
(the program), meaning future instructions in this same program
can update the metadata (level up).

## 5. The metadata-pointer extension in detail

Traditional Metaplex NFT:

```
mint (SPL-Token)       ← name/symbol/uri NOT here
metadata PDA (Metaplex) ← name, symbol, uri
edition PDA (Metaplex)  ← edition state
```

Three accounts, two programs.

Token-2022 with `MetadataPointer` + `TokenMetadata` extensions:

```
mint (Token-2022)
 ├── core mint fields
 ├── TLV: MetadataPointer { authority, address = self }
 └── TLV: TokenMetadata { name, symbol, uri,
                          additional_metadata: [(k, v), ...] }
```

One account, one program. Cheaper, simpler, and custom fields are
first-class.

### Why `metadata_address == mint_address`?

It means "the metadata is embedded in this mint". Clients reading
the mint already have the metadata — no second RPC call needed.
If you wanted versioned metadata or shared metadata across mints,
you could point `metadata_address` at a separate account.

### `additional_metadata`

A `Vec<(String, String)>`. The example writes `("level", "1")`.
You can write anything: `("xp", "420")`, `("weapon",
"sword_lv3")`, etc. Marketplaces that understand Token-2022
metadata can display and sort on these fields — the long-term
vision of the extension.

## 6. The lazy-energy pattern

Common in casual games. Instead of a scheduled task pushing energy
to every player every minute (impossibly expensive onchain), each
transaction catches up on the regen owed since `last_login`.

```
# Client-side display (runs every second in the UI)
elapsed = now - player.last_login
regen = min(MAX_ENERGY - player.energy, elapsed // TIME_TO_REFILL_ENERGY)
display_energy = player.energy + regen

# Server-side (runs in update_energy during chop_tree)
while time_passed >= TIME_TO_REFILL_ENERGY and energy < MAX_ENERGY:
    energy += 1
    time_passed -= TIME_TO_REFILL_ENERGY
    time_spent += TIME_TO_REFILL_ENERGY
```

Client and server agree: the client's prediction becomes the
server's truth as soon as the next onchain action runs. No polling
needed — websocket account subscriptions push the update after a
successful chop.

## 7. Session keys

Without session keys, every `chop_tree` would need a wallet popup.
That's fine for "send 10 SOL" but maddening for a game where
actions are per-second.

**Flow:**

1. Client generates a local keypair `S`.
2. Client sends one transaction, signed by the player's main
   wallet, creating a session token that whitelists `chop_tree`
   on this program, expires in 23h, signed by `S`.
3. Client signs subsequent `chop_tree` calls with `S` (no prompt).
4. The program's `#[session_auth_or]` macro checks the session
   token is live, the instruction is whitelisted, and `S` is the
   signer.
5. On expiry, remaining SOL in `S` flows back to the main wallet.

Maintained by Magic Block / Gum — see
[sessionkeys](https://docs.magicblock.gg/session-keys/get-started).
Treat session keys as unaudited third-party code for now.

## 8. Worked example

```
t=0:    Alice calls init_player.
        player.authority=Alice, energy=10, wood=0, last_login=t0.

t=60:   Alice calls chop_tree(counter=1).
        update_energy: time_passed=60. Loop: energy stays 10 (cap).
        energy -= 1 → 9. wood += 1 → 1. last_login=t0+60.

t=600:  Alice calls chop_tree 9 more times.
        energy → 0. wood → 10.

t=660:  Alice calls chop_tree again.
        update_energy: time_passed=60 since last login.
        Loop: 1 refill. energy=1. last_login moves forward 60s.
        energy -= 1 → 0. wood += 1 → 11.

t=720:  Alice calls mint_nft.
        - Mint M created: Token-2022, 0 decimals, MetadataPointer
          → self.
        - Embedded metadata: name="Beaver", symbol="BVA",
          uri="https://arweave.net/MHK3Iopy...".
        - additional_metadata: {"level": "1"}.
        - Alice's ATA gets 1 unit.
        - mint authority → None. Supply locked at 1 forever.
```

A (hypothetical) `level_up` instruction would CPI `update_field(M,
"level", "2")` signed by `nft_authority`. The NFT's metadata
changes onchain; marketplaces reading the mint see the new level
immediately.

## 9. Safety and edge cases

- **Session key trust.** The session-key crate is third-party and
  unaudited at the time of writing. For high-value actions, keep
  the main wallet as the authority.
- **Extension order.** Extensions must be initialised *before*
  the mint itself. Swap the CPIs and you get an obscure error
  from `spl-token-2022`.
- **`meta_data_space = 250` is a guess.** It's enough for the
  current name/symbol/uri + one small field. If you add many
  custom fields, increase this or use `realloc`.
- **ATA for Token-2022.** The associated-token program routes
  correctly if you pass Token-2022 as `token_program`. Mixing SPL
  Token and Token-2022 ATAs for the same mint is impossible —
  they'd have different addresses.
- **Mint authority revoked on `mint_nft`.** Irreversible. No one
  can ever mint more of this NFT.
- **`update_field` size bumps.** Adding a long string to
  `additional_metadata` can exceed the mint's current account
  size. The CPI errors; you'd need to `realloc` first (not done
  in this example).
- **`nft_authority` PDA is global.** Seeds `["nft_authority"]`
  with no variable component → one PDA per program, used for every
  NFT. So a player can't freeze their metadata by "removing"
  this authority — nobody holds its key.
- **`player.authority` check bypass.** `#[session_auth_or]`
  explicitly allows either the session key or the main authority
  through. If session-key code has a bug, the fallback still
  works.

## 10. Running the example

### Anchor program

```bash
cd anchor
anchor build
anchor deploy   # local or devnet
anchor test --detach
```

After deploy, copy the program id into `anchor/Anchor.toml`,
`anchor/programs/extension_nft/src/lib.rs`, `app/utils/anchor.ts`,
and the Unity AnchorService. Rebuild and redeploy if you change
the id.

### TypeScript client (Next.js)

```bash
cd app
yarn install
yarn dev
# open http://localhost:3000
```

### Unity client

Open the Unity project with Unity 2021.3.32f1 or similar. Open
`GameScene` or `LoginScene` and hit Play. Use the editor login
button (bottom-left) to set up a test wallet.

Regenerating the C# client after program changes:

```bash
dotnet tool install Solana.Unity.Anchor.Tool  # once
cd anchor
dotnet anchorgen -i target/idl/extension_nft.json -o target/idl/ExtensionNft.cs
# then copy the generated file into the Unity project
```

## 11. Extending the program

- **Level up.** Add `level_up()` that increments
  `player.level` and CPIs `update_field(mint, "level",
  player.level.to_string())`.
- **More fields.** Track `xp`, `weapon`, `last_kill_timestamp`.
  Anything string-encodable.
- **Per-rarity mints.** Weight the NFT's attributes on mint (roll
  a random stat set, store in metadata).
- **Collection support.** Add a `Collection` extension pointing
  at a collection mint; verify in a follow-up instruction.
- **Royalties.** Use the `TransferFee` extension to take a fee
  on every transfer.
- **Audit session keys.** If you ship for real, pin a specific
  session-keys version and read its source. Unaudited crates in
  mainnet authentication logic are a bad idea.

## References

- [Token-2022 metadata pointer docs](https://spl.solana.com/token-2022/extensions#metadata-pointer)
- [Solana Foundation gaming playlist](https://www.youtube.com/@SolanaFndn/videos)
- [Session keys (Magic Block)](https://docs.magicblock.gg/session-keys/)
- [Anchor to Unity](https://solanacookbook.com/gaming/porting-anchor-to-unity.html)
