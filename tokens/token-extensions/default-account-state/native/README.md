# Default Account State (Token-2022)

A tiny native program that creates a Token-2022 mint with the
`DefaultAccountState` extension set to `Frozen`, initialises the
mint, then updates the default state to `Initialized`. Minimum
viable demonstration of the extension's lifecycle.

The extension lets a mint creator define the default state of every
new token account created for that mint — either `Initialized`
(normal) or `Frozen` (holders can't transfer until the freeze
authority thaws them). Useful for KYC-gated or permissioned tokens.

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

One instruction (default handler, no discriminator — the program
has only one). Given `CreateTokenArgs { token_decimals: u8 }`:

1. Allocate enough bytes for a `Mint` + `DefaultAccountState`
   extension. Payer funds the rent.
2. CPI `initialize_default_account_state(mint, AccountState::Frozen)`.
3. CPI `initialize_mint(mint, decimals, mint_authority,
   freeze_authority = mint_authority)`.
4. CPI `update_default_account_state(mint, Initialized, signed by
   freeze authority)`.

The end result: a Token-2022 mint that has the extension configured,
but currently defaults new accounts to `Initialized` (normal). Steps
2 and 4 together demonstrate both ends of the extension's API —
real apps usually leave it at `Frozen` and thaw per-user.

## 2. Glossary

**Token-2022 (`spl-token-2022`)**
: The newer SPL token program
(`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`). Wire-compatible
with SPL Token for basic mint/transfer but adds **extensions** —
optional extra features activated per-mint.

**Extension**
: An opt-in modifier on a Token-2022 mint or account. Encoded as
TLV (type-length-value) bytes appended after the mint's core
fields. You select extensions at mint creation; they can't be
added later.

**`DefaultAccountState`**
: The extension in play here. Stores a single `AccountState` byte.
When a new token account is created for this mint, its `state` is
initialised from this value. Value can be `Initialized`,
`Frozen`, or `Uninitialized`. Changing the default requires the
mint's freeze authority.

**`AccountState`**
: An enum on every token account — `Uninitialized | Initialized |
Frozen`. `Frozen` blocks transfers / burn / approve until thawed.

**Freeze authority**
: The pubkey allowed to freeze and thaw individual token accounts
(via `freeze_account` / `thaw_account` instructions). For
Token-2022, it's also required to change the default account
state. If there is no freeze authority, the extension can't be
updated.

**`ExtensionType::try_calculate_account_len`**
: Helper from the Token-2022 interface crate that returns the
byte size of a Mint with a specified set of extensions. Avoids
hand-computing TLV layout.

## 3. Accounts and PDAs

| name | kind | stores | who signs |
|---|---|---|---|
| `mint_account` | keypair, init | Token-2022 Mint with `DefaultAccountState` ext | mint keypair |
| `mint_authority` | signer | — | mint authority (also freeze authority) |
| `payer` | signer, mut | SOL (pays rent + fee) | user |
| `rent` | sysvar | — | — |
| `system_program`, `token_program` (Token-2022) | programs | — | — |

Note: there's a duplicate responsibility here — the example passes
`mint_authority` (signer) for `initialize_mint` and `payer` (signer)
for `update_default_account_state`. In the test fixture these are
often the same wallet. See §6.

## 4. Instruction lifecycle walkthrough

### Default handler: `create_token(decimals)`

**Who calls it:** anyone with SOL + a fresh mint keypair.

**Signers:** `mint_account` (new keypair), `mint_authority`,
`payer`.

**Order matters** — Token-2022 requires extensions to be initialised
**before** the mint itself. Get this wrong and the CPI errors.

**Step by step:**

1. Compute extension-aware space:
   ```rust
   ExtensionType::try_calculate_account_len::<Mint>(
       &[ExtensionType::DefaultAccountState])
   ```
   Returns size of core `Mint` + TLV overhead + 1 byte for the
   extension's state.
2. Get `Rent::get()?.minimum_balance(space)` lamports.
3. `invoke(system::create_account(from=payer, to=mint, lamports,
   space, owner=token_program))` — allocates the account under
   Token-2022 ownership.
4. `invoke(initialize_default_account_state(mint,
   AccountState::Frozen))` — installs the extension in `Frozen`
   state. Must be before `initialize_mint`.
5. `invoke(initialize_mint(mint, decimals, mint_authority, Some(mint_authority)))`
   — core mint initialisation. The freeze authority is set to
   `mint_authority` so it can also update the default state
   later.
6. `invoke(update_default_account_state(mint, Initialized, authority
   = payer, signers = [payer]))` — changes the default to
   `Initialized`.

**Token movements:** none. No supply at this point.

**State changes:** new mint created with TLV, extension configured.

**Checks:**
- System program ensures mint address isn't reused.
- Token-2022 checks authorities on each subsequent call (mint
  keypair for init, freeze authority for default-state updates).

## 5. Worked example

```
1. Alice calls the program with:
     - mint_account = fresh keypair K
     - mint_authority = Alice
     - payer = Alice
     - token_program = Token-2022 id
     - decimals = 9

2. After the instruction:
     Mint @ K:
       decimals = 9
       mint_authority = Alice
       freeze_authority = Alice
       supply = 0
       extensions: [DefaultAccountState { state: Initialized }]

3. Someone later creates an ATA for K (e.g. for Bob).
   - Bob's new ATA starts in state `Initialized`.
   - Normal. Transfers work.

4. If step 6 in the lifecycle had been left at `Frozen` (or Alice
   calls update_default_account_state(Frozen) again later):
   - Any newly-created ATA for K starts in state `Frozen`.
   - Bob can receive tokens via mint (some Token-2022 ops skip
     the frozen check), but can't transfer them out.
   - Alice (freeze authority) must call `thaw_account` before
     Bob can move them.
```

## 6. Safety and edge cases

- **Order of extension init.** Extensions must be initialised
  between `create_account` and `initialize_mint`. Swap those two
  CPIs and the mint init fails because the TLV isn't ready.
- **Freeze authority required.** You must pass
  `Some(mint_authority)` as the freeze authority in
  `initialize_mint`. If you pass `None`, subsequent calls to
  `update_default_account_state` fail — nobody can change it.
- **`payer` signs the state update here.** The code passes `payer`
  as the authority for `update_default_account_state`. That only
  works if `payer == freeze_authority`. In this example
  `mint_authority` is Alice and `payer` is Alice, so fine. If you
  separate the roles, pass the freeze authority there instead.
- **Frozen default + mint-to.** Minting tokens into a frozen
  account actually *works* in Token-2022 (mint_to bypasses the
  frozen check). Transfers out still don't. This lets a compliance
  workflow "pre-issue" tokens while holders are still being
  verified.
- **Extension not upgradable.** Once the mint exists with this
  extension, you can't remove it. You can only update its state
  (Initialized ↔ Frozen).
- **Size mismatch.** If you miscalculate `space` (e.g. omit
  `DefaultAccountState` from the list), the mint init will refuse
  to install the extension because the account is too small.

## 7. Running the tests

```bash
cd native
cargo build-sbf
pnpm install
pnpm test
```

The tests (TypeScript) deploy the program, call `create_token`,
then assert the mint has the extension configured and verify its
default state.

## 8. Extending the program

- **Leave default as `Frozen`.** Drop the step-6 update and
  require an explicit `thaw_account` per user, implementing a
  real KYC-style flow.
- **Add a KYC PDA.** Store a per-user KYC status PDA; expose a
  `thaw_if_kyc_passed` instruction that the freeze authority runs,
  verifying the KYC PDA before thawing.
- **Switch default at runtime.** Add an admin-gated instruction
  that toggles Initialized ↔ Frozen on demand (e.g. to pause all
  new onboarding).
- **Anchor version.** Port to Anchor using
  `anchor-spl::token_interface`. Much less CPI boilerplate.
- **Combine with `transfer-hook`.** Default-frozen + a transfer
  hook gives you "compliance NFT" behaviour — holders can transfer
  only when both the freeze state is clear and the hook approves.
