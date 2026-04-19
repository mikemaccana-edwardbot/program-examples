# Allow / Block List Token (Transfer Hook)

A Token-2022 mint with the `TransferHook` extension pointing at
this program. Every transfer of the mint triggers a CPI into the
hook, which checks the recipient against an onchain allow/block
list and either approves or rejects the transfer.

Three list modes, configured on the mint's embedded metadata:

- **Allow** — recipient must be in the list, or the transfer fails.
- **Block** — anyone except listed addresses can receive.
- **Mixed (threshold)** — anyone can receive up to a threshold;
  amounts at or above the threshold require the recipient to be
  explicitly allowed.

The list is stored in this program's state and can be **shared
across multiple mints** — a single compliance-list manager can
serve a whole family of tokens.

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [The transfer-hook call chain](#5-the-transfer-hook-call-chain)
6. [Worked example](#6-worked-example)
7. [Safety and edge cases](#7-safety-and-edge-cases)
8. [Running the tests and UI](#8-running-the-tests-and-ui)
9. [Extending the program](#9-extending-the-program)

## 1. What does this program do?

Seven instructions:

| instruction | who | effect |
|---|---|---|
| `init_config` | anyone | Creates a `Config` PDA storing `authority` — the wallet allowed to manage the list. |
| `init_mint(args)` | any issuer | Creates a Token-2022 mint with `TransferHook`, `MetadataPointer`, `TokenMetadata` and (optional) `PermanentDelegate` extensions. The transfer-hook program is set to this one. |
| `attach_to_mint` | mint authority | Runs `TransferHookInstruction::InitializeExtraAccountMetaList` so the Token-2022 program knows which extra accounts to pass to the hook on each transfer. |
| `init_wallet(args)` | config authority | Creates an `ABWallet` PDA seeded by `["ab_wallet", wallet]`, storing `{ wallet, allowed: bool }`. |
| `remove_wallet` | config authority | Closes an `ABWallet` PDA, refunding rent. |
| `change_mode(args)` | mint authority | Updates the `"AB"` key in the mint's `additional_metadata` to `Allow` / `Block` / `Mixed:<threshold>`. |
| `tx_hook(amount)` | Token-2022 (via CPI) | Executed during a transfer. Reads the mint's `"AB"` metadata + the recipient's `ABWallet` (or empty) + the amount, returns `Ok` or `Err`. |

## 2. Glossary

**Transfer hook (Token-2022 extension)**
: An extension that names another program to CPI into on every
transfer of this mint. The Token-2022 program calls
`<hook_program>::Execute(amount)` during `transfer_checked`,
passing the source and destination token accounts plus "extra
accounts" the hook needs. If the hook returns `Err`, the transfer
reverts.

**`Execute` instruction**
: The required entrypoint of every transfer-hook program.
Defined by the `spl-transfer-hook-interface` crate. Has a
specific 8-byte discriminator (`ExecuteInstruction::SPL_DISCRIMINATOR`),
which this program uses via
`#[instruction(discriminator = ExecuteInstruction::SPL_DISCRIMINATOR_SLICE)]`.
That's how our `tx_hook` function binds to the standard.

**`ExtraAccountMetaList`**
: A companion account (stored at seeds `["extra-account-metas",
mint]`) that the Token-2022 program reads before each transfer to
know which extra accounts to pass into the hook. `attach_to_mint`
creates and populates it.

**`MetadataPointer` + `TokenMetadata`**
: Token-2022 extensions storing metadata inline in the mint. The
`additional_metadata` key/value list holds the `"AB"` entry that
controls which list mode is active.

**`PermanentDelegate` extension**
: Optional per-mint extension giving a specific pubkey unconditional
transfer authority — even over frozen accounts. Useful for
compliance-recovery ("admin can seize stolen tokens"). Set in
`init_mint` via `args.permanent_delegate`.

**Allow / Block / Mixed (threshold)**
: The three modes the mint's `"AB"` metadata can take.
- `Allow`: transfer only if recipient has an `ABWallet` PDA with
  `allowed = true`.
- `Block`: transfer unless recipient has `allowed = false`.
- `Mixed:N`: transfer always below `N`, require `allowed = true`
  at or above.

**`ABWallet` PDA**
: Seeds `["ab_wallet", recipient]`. Holds `{ wallet, allowed }`.
Missing PDA = "not on any list".

**Hook authority**
: The pubkey allowed to update the mint's transfer-hook
configuration (e.g. change which program the hook points at).
Passed at mint creation as `args.transfer_hook_authority`.

## 3. Accounts and PDAs

### Per-program

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `config` | PDA | `["config"]` | `Config { authority, bump }` | — |

### Per-mint (created by issuer)

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `mint` | Token-2022 mint | — | base mint + TransferHook + MetadataPointer + TokenMetadata (+ PermanentDelegate if set) | payer (at creation) |
| `extra_metas_account` | PDA | `["extra-account-metas", mint]` | list of extra accounts the hook needs | — |

### Per-recipient (managed by config authority)

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `ab_wallet` | PDA | `["ab_wallet", wallet]` | `{ wallet, allowed }` | — |

## 4. Instruction lifecycle walkthrough

### `init_config()`
Creates `Config` PDA. First-come-first-served: the first caller
becomes the list authority. Run once per deployment.

### `init_mint(InitMintArgs)`
Creates a Token-2022 mint with four extensions configured declaratively
via Anchor 0.32's extension-attribute syntax:

```rust
extensions::permanent_delegate::delegate = args.permanent_delegate,
extensions::transfer_hook::authority = args.transfer_hook_authority,
extensions::transfer_hook::program_id = crate::id(),
extensions::metadata_pointer::authority = payer.key(),
extensions::metadata_pointer::metadata_address = mint.key(),
```

The `ExtraAccountMetaList` PDA is also allocated here, ready to be
populated by `attach_to_mint`.

Then CPIs into Token-2022's metadata interface to write:
- `name`, `symbol`, `uri` (from args)
- `additional_metadata: [("AB", <"Allow"|"Block"|"Mixed:N">)]`

### `attach_to_mint()`
CPIs `InitializeExtraAccountMetaList` on this program (no — it's
the Token-2022 program's transfer-hook interface helper).
Populates the extra-metas PDA with the list of accounts Token-2022
must pass into `tx_hook` during every transfer. For this program,
that's the `ABWallet` PDA for the destination (derived from the
destination token account's owner).

### `init_wallet(args)`
Config authority creates an `ABWallet` PDA for a specific
recipient, with `allowed = args.allowed`.

### `remove_wallet()`
Config authority closes an `ABWallet`, refunding rent.

### `change_mode(args)`
Mint authority updates the mint's `"AB"` metadata field to the
chosen `Mode`.

### `tx_hook(amount)` — the gatekeeper

Called by the Token-2022 program *inside* every `transfer_checked`.

**Accounts (all `UncheckedAccount`):**
- `source_token_account`, `mint`, `destination_token_account`,
  `owner_delegate`, `meta_list`, `ab_wallet`.

**Behaviour:**

1. Unpack the mint: `StateWithExtensions::<Mint>::unpack(&mint_data)`.
2. Read its `TokenMetadata` extension, scan
   `additional_metadata` for `"AB"`. Parse into
   `DecodedMintMode::{Allow, Block, Threshold(n)}`.
3. Read the destination `ABWallet`. If the account is empty
   (PDA never initialised), mode is `None`. Otherwise
   `{Allow, Block}` based on `allowed`.
4. Match:
   - Allow × Allow → Ok
   - Allow × (anything else) → Err(WalletNotAllowed)
   - Anything × Block → Err(WalletBlocked)
   - Block × (not Block) → Ok
   - Threshold(n) × None, amount >= n → Err(AmountNotAllowed)
   - Threshold(n) × _ → Ok (below threshold, or explicitly
     allowed)
5. Return.

Token-2022 interprets the return value: `Ok` → transfer proceeds,
`Err` → transfer aborts and the caller sees the error.

## 5. The transfer-hook call chain

Every user-initiated transfer of this mint flows like this:

```
user / dapp
 └── Token-2022::transfer_checked
      ├── amount, mint, decimals checks
      ├── permanent_delegate / freeze checks
      └── transfer_hook extension present →
           └── CPI: abl_token::Execute / tx_hook(amount)
                └── reads mint metadata, destination ABWallet
                └── returns Ok or Err
      ├── if hook Err → abort
      └── else → apply balance changes
```

The interesting pieces from the user's perspective:

- They never call `tx_hook` directly. Even when building the
  transaction, they let Token-2022 + Anchor's extra-metas
  resolver assemble the hook's account list for them (on
  mainnet). On devnet and localnet most wallets can't resolve
  extra metas, which is why this repo ships a UI that builds the
  accounts manually.
- The hook sees the raw `amount` in base units (not the UI
  decimal-adjusted amount). Threshold comparisons must use raw
  units.

## 6. Worked example

```
Setup:
  Deployer runs init_config.
  Alice runs init_mint for USD-Compliance (USDc) with:
    - mode = Mixed:100_000_000 (100 USDc below threshold; 100+ needs allow)
    - transfer_hook program = abl_token
    - permanent_delegate = Alice (for seizure)
  Alice runs attach_to_mint.
  Alice runs init_wallet for Charlie with allowed = true
    (so Charlie can receive big transfers).

Flow 1 — small transfer (below threshold):
  Alice transfers 50 USDc to Bob.
  Token-2022 CPIs tx_hook(amount = 50_000_000).
  tx_hook reads:
    mint metadata: AB = "Mixed:100000000"
    Bob's ABWallet: doesn't exist → None.
  Match: Threshold(100M) × None, amount < threshold → Ok.
  Transfer proceeds.

Flow 2 — big transfer to un-listed recipient:
  Alice transfers 200 USDc to Dave.
  tx_hook(200_000_000):
    mint: Threshold(100M).
    Dave's ABWallet: None.
  Match: Threshold × None, amount ≥ threshold → Err(AmountNotAllowed).
  Transfer reverts.

Flow 3 — big transfer to Charlie:
  Alice transfers 500 USDc to Charlie.
  tx_hook(500_000_000):
    mint: Threshold(100M).
    Charlie's ABWallet: allowed = true → Allow.
  Match: Threshold × Allow → Ok.
  Transfer proceeds.

Flow 4 — block-list lookup:
  Alice changes mode to Block (change_mode).
  Alice init_wallets for Eve with allowed = false.
  Any Alice → Eve transfer now fails with WalletBlocked regardless
  of amount.
```

## 7. Safety and edge cases

- **Wallet resolution on clients.** Most wallets still don't
  auto-resolve the extra accounts a transfer hook requires,
  especially on devnet. That's why this repo ships its own UI.
  On mainnet with wallet-adapter `>=`0.9.35, it works via the
  wallet's `additional_accounts_for_transfer` resolver.
- **`Execute` discriminator.** Transfer hooks are identified by
  their discriminator, *not* by a named method. If you rename
  `tx_hook` or forget the
  `#[instruction(discriminator = ExecuteInstruction::SPL_DISCRIMINATOR_SLICE)]`
  attribute, Token-2022 will be unable to find your hook
  entrypoint.
- **`UncheckedAccount` everywhere in `TxHook`.** The hook
  trusts what Token-2022 passes. The `ab_wallet` account may be
  uninitialised (PDA doesn't exist yet) — code explicitly handles
  `data_is_empty()`. Don't add deserialisation constraints here;
  they'd break the "wallet not on any list" case.
- **Permanent delegate.** Granting this is irreversible. The
  delegate can transfer any balance of the mint, bypassing the
  usual owner-signing. Pair with compliance flows only.
- **Threshold parsing.** `Mixed:N` is parsed from the metadata
  string. Malformed input (e.g. `"Mixed:abc"`) errors with
  `InvalidMetadata`. Integrate carefully with the UI.
- **Config authority is a single key.** For real deployments
  consider a multisig or governance PDA as the config authority.
- **Shared lists across mints.** The same `ABWallet` PDA applies
  to every mint that points its `TransferHook` at this program.
  That's intentional — the whole point is cross-mint sharing —
  but means revoking one wallet hits all mints at once. Use
  per-authority configs if you want stricter separation.

## 8. Running the tests and UI

```bash
# 1. Install front-end + program deps
yarn install

# 2. Compile the program (update declare_id! after first build)
anchor build

# 3. Local validator + deploy
./scripts/start.sh        # starts solana-test-validator and deploys

# 4. UI
yarn run build
yarn run dev              # http://localhost:3000

# 5. Stop
./scripts/stop.sh
```

### Rust integration tests

The Rust tests live in `anchor/tests-rs/` instead of the usual
`programs/abl-token/tests/` path because of an unresolvable
`solana-account-info` version conflict between `litesvm` and
`anchor-lang` 0.32. See [`tests-rs/README.md`](anchor/tests-rs/README.md)
for the detailed pinning situation. Once either Anchor upgrades to
solana 3.x or litesvm releases a solana-2.3-compatible line, move
the file back and restore the `[dev-dependencies]` section.

### TypeScript tests

Standard Anchor TS tests live in `anchor/tests/`. Run via
`anchor test` once the program is deployed locally.

## 9. Extending the program

- **Per-mint lists.** Add `mint: Pubkey` to `ABWallet` seeds:
  `["ab_wallet", mint, wallet]`. Turns the shared list into a
  per-mint list while keeping the rest of the logic the same.
- **Per-wallet limits.** Store a max-transfer-size on each
  `ABWallet`; check against `amount` in the Threshold branch.
- **Expiry.** Add `expires_at: i64` to `ABWallet`; treat the
  wallet as "None" after expiry. Supports time-limited KYC.
- **Hook-triggered events.** Emit an Anchor event for each
  denied transfer so indexers can surface compliance attempts.
- **Program-owned list.** Gate `init_wallet` / `remove_wallet` on
  a multisig PDA instead of a single authority. Reduces key-loss
  risk.
- **UI polish.** The current UI is based on the
  `legacy-next-tailwind-basic` Anchor template. Swap in a real
  design once you've got the flows working.
