# PDA Rent Payer

A tiny program that funds a PDA "vault" with SOL and then uses that
vault to pay the rent for new accounts. The vault signs the create
transactions using its program-derived seeds — no user signature
required for rent.

The neat trick this illustrates: on Solana, transferring lamports to
a brand-new address is enough to create a System-owned account
there. And a PDA can sign such transfers, so you can fund arbitrary
accounts on behalf of users.

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

Two instructions:

1. **`init_rent_vault(fund_lamports)`** — anyone tops up the vault
   PDA with SOL via a System program transfer CPI.
2. **`create_new_account()`** — the program CPIs into the System
   program to create a new 0-byte, System-owned account at the
   caller's chosen address, funded from the vault. The vault signs
   as a PDA using `invoke_signed` (expressed as
   `CpiContext::new(...).with_signer(signer_seeds)` in Anchor).

No state accounts beyond the vault itself — which is just a
SystemAccount, no custom data.

## 2. Glossary

**PDA (Program-Derived Address)**
: An address derived from seeds + program id that sits off the
ed25519 curve. PDAs have no private key; the owning program "signs"
by calling `invoke_signed` with the same seeds. Here the vault's
seeds are just `["rent_vault"]`, so there's exactly one vault per
program.

**System account / System program**
: The System program (`11111111111111111111111111111111`) is the
built-in program that owns every SOL wallet. A "System account" is
an account whose owner is the System program — it holds SOL and
zero bytes of user data. `SystemAccount<'info>` is Anchor's typed
wrapper for this.

**Rent-exempt minimum**
: The minimum lamports an account needs to keep forever, based on
its size. A 0-byte account needs ~890 880 lamports (≈ 0.00089 SOL).
See `basics/rent` for the details.

**"Creating an account by transfer"**
: When you transfer lamports to an address that has no existing
account, Solana automatically creates a System-owned, 0-byte account
at that address. So `System::transfer(to = newaddr, lamports =
rent_min)` is functionally the same as a `create_account` CPI for a
0-byte System-owned account — which is what `create_new_account`
leverages.

**`invoke_signed`**
: The runtime call that lets a program sign for a PDA by passing its
seeds. In Anchor, `CpiContext::new_with_signer(program, accounts,
signer_seeds)` or
`CpiContext::new(...).with_signer(signer_seeds)` wraps it.

## 3. Accounts and PDAs

| name | kind | seeds | stores | who signs |
|---|---|---|---|---|
| `rent_vault` | PDA, System-owned | `["rent_vault"]` | SOL (nothing else) | program (via seeds) during create; System program during top-up |
| `payer` (top-up) | signer, mut | SOL | user |
| `new_account` | signer, mut | SOL (0-byte System account after creation) | user |
| `system_program` | program | — | — | — |

There is exactly **one** vault per deployed instance of this program
(the seeds `["rent_vault"]` don't depend on anything).

## 4. Instruction lifecycle walkthrough

### `init_rent_vault(fund_lamports: u64)`

**Who calls it:** anyone willing to donate SOL to the vault.

**Signers:** `payer`.

**Accounts in:**
- `payer` (signer, mut)
- `rent_vault` (PDA, mut) — seeded `["rent_vault"]`
- `system_program`

**Behaviour:** CPI `system::transfer(from=payer, to=rent_vault,
lamports=fund_lamports)`. If the vault address didn't have an
account yet, this transfer creates it (System-owned, 0 bytes). If it
did, the lamports are added to the existing balance.

**Token movements:**

```
payer (wallet) --[fund_lamports]--> rent_vault PDA (seeds: ["rent_vault"])
```

**State changes:** vault balance increases. No custom data.

**Checks:** PDA address derived from seeds must match what the
caller passed. Nothing restricts *who* may top up.

### `create_new_account()`

**Who calls it:** anyone with a fresh keypair to seed the new
account.

**Signers:** `new_account` (the new keypair signs its own creation).

**Accounts in:**
- `new_account` (signer, mut) — the address to bring into being
- `rent_vault` (PDA, mut) — source of funds
- `system_program`

**Behaviour:**
1. Compute rent-exempt minimum for a 0-byte account
   (`Rent::get()?.minimum_balance(0)`).
2. Build CPI `system::create_account(from=rent_vault,
   to=new_account, lamports=<min>, space=0, owner=system_program)`
   with signer seeds `[["rent_vault", bump]]`.
3. System program deducts lamports from vault, allocates new
   account, sets owner = System program.

**Token movements:**

```
rent_vault PDA --[rent_exempt_minimum]--> new_account (System-owned, 0 bytes)
```

**State changes:** `new_account` goes from nonexistent to existing.
Vault balance drops by ~890 880 lamports.

**Checks:**
- `new_account` must sign.
- `rent_vault` PDA must match `find_program_address(["rent_vault"],
  program_id)` — Anchor checks this via the `seeds` constraint.
- Vault must have enough SOL; otherwise the System program returns
  `InsufficientFunds`.

## 5. Worked example

```
1. Alice calls init_rent_vault(fund_lamports = 10_000_000).
   - 0.01 SOL moves from Alice to rent_vault PDA.
   - Vault balance: 10_000_000.

2. Bob generates keypair K (address Kx..).
   Bob calls create_new_account, signing as new_account=K.
   - Program CPIs system::create_account(from=vault, to=Kx..,
     lamports=890_880, space=0, owner=system).
   - Vault's PDA signature: invoke_signed with seeds=["rent_vault"].
   - Kx.. now exists: 0 bytes, System-owned, 890_880 lamports.
   - Vault balance: 10_000_000 - 890_880 = 9_109_120.

3. Bob did not pay anything except the transaction fee.
   Alice funded his account creation.
```

## 6. Safety and edge cases

- **Anyone can drain the vault via `create_new_account`.** The
  program doesn't check *who* is calling. That's fine for a demo but
  is a DoS / griefing vector in production: an attacker spams
  `create_new_account` with fresh keypairs and exhausts the vault.
  Fix: gate the call with an authority or attach an application-level
  policy (e.g. only signable by a privileged caller, or rate-limited
  off-chain).
- **Vault has no rent itself.** It starts as "doesn't exist" until
  first funded. If you ever drain it below the rent-exempt minimum
  for 0 bytes (it has no data so effectively 0), the System program
  will garbage-collect it on the next epoch. Keeping a small buffer
  in the vault avoids surprises.
- **Fixed single vault.** Seeds are `["rent_vault"]`, no per-user
  vaults. If you want sharding or per-app vaults, add more seeds.
- **Vault is System-owned, not program-owned.** That's the clever
  part — the program can still sign for it because PDA authority is
  derived from seeds + program id, not from ownership. This means
  you cannot store custom data in the vault; it's strictly a SOL
  bucket.
- **Reused new_account address.** If `new_account` already exists,
  System::create_account fails with `AccountAlreadyInUse`.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native / Pinocchio
cd native && cargo build-sbf && cargo test
cd pinocchio && cargo build-sbf && cargo test
```

The Anchor tests call `init_rent_vault` then `create_new_account`
in sequence and assert the new account exists with the expected
owner and lamport balance.

## 8. Extending the program

- **Gate `create_new_account`.** Add an `authority: Pubkey` stored
  in a separate config PDA; reject calls from other signers.
- **Charge the caller something.** Instead of free account creation,
  have the caller pay a fee to the vault on create — useful for a
  "faucet with margin" pattern.
- **Allocate more than 0 bytes.** Take `space: u64` and
  `owner: Pubkey` as args; call `system::create_account` with those
  values. Let the vault bankroll arbitrary account creation for some
  external program.
- **Withdraw from vault.** Add a `drain_vault` instruction signed by
  an admin key that returns lamports to a specified destination.
  Useful for upgrades / migrations.
- **Multiple vaults.** Seed with `["rent_vault", authority_pubkey]`
  so each authority has their own vault.
