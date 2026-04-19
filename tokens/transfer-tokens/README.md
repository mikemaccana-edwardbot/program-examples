# Transfer Tokens

Three instructions: `create_token`, `mint_token` (both identical to
`tokens/spl-token-minter`), and `transfer_tokens(amount)` which
moves tokens from one ATA to another via `token::transfer`.

The point of this example is the last instruction: showing the
happy path for an SPL-Token transfer mediated by your own program.
In most cases you don't *need* a program to transfer SPL tokens —
clients can call the SPL Token program directly. Doing it through
your program is useful when you want to add custom rules (fees,
whitelists, compliance, etc.) around the transfer.

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

- `create_token(title, symbol, uri)` — same as `spl-token-minter`:
  new SPL mint (9 decimals) + Metaplex metadata.
- `mint_token(amount)` — mints `amount × 10^9` units into the
  sender's ATA.
- `transfer_tokens(amount)` — CPIs `token::transfer` to move
  `amount × 10^9` from the sender's ATA to the recipient's ATA.
  If the recipient has no ATA yet, it's created (payer = sender).

## 2. Glossary

**SPL Token transfer**
: The SPL Token program's `transfer` (or the newer
`transfer_checked`) instruction. Moves balances between two token
accounts that share a mint. Requires the source's authority (owner
or delegate) to sign.

**ATA (Associated Token Account)**
: The canonical token account for a (mint, owner) pair. Derived
address — no database lookup needed. See
`tokens/spl-token-minter` glossary for the full derivation.

**`init_if_needed`**
: Anchor constraint that creates the account if absent. Used here
so the first transfer to a fresh recipient also creates their ATA
(paid for by the sender).

**`transfer` vs `transfer_checked`**
: `transfer` (used here) takes `amount` only. `transfer_checked`
additionally takes `mint` and `decimals` and verifies both.
`transfer_checked` is recommended in new code — it catches the
common bug of sending to a wrong-mint ATA. This example uses the
older `transfer` for simplicity.

**Authority on transfer**
: Every token transfer requires the source account's authority to
sign. By default the authority is the account's `owner` field.
`token::approve` can delegate transfer rights to another pubkey
without giving up ownership.

## 3. Accounts and PDAs

No PDAs. Everything is a regular SPL token account or wallet
keypair.

### `transfer_tokens`

| name | kind | stores | who signs |
|---|---|---|---|
| `sender` | signer, mut | SOL (pays tx fee + rent for recipient ATA if created) | user |
| `recipient` | system account | — | — |
| `mint_account` | mint, mut | mint data (not actually modified here, but Anchor includes it for decimal lookup) | — |
| `sender_token_account` | ATA (mint, sender), mut | sender's balance | — (sender signs as ATA authority) |
| `recipient_token_account` | ATA (mint, recipient), init_if_needed | recipient's balance | — |
| `token_program`, `associated_token_program`, `system_program` | programs | — | — |

## 4. Instruction lifecycle walkthrough

### `transfer_tokens(amount: u64)`

1. If the recipient's ATA is absent, Anchor creates it (payer =
   sender, rent ~0.002 SOL).
2. CPI `token::transfer(from = sender_ata, to = recipient_ata,
   authority = sender, amount = amount × 10^decimals)`.

**Token movements:**

```
sender ATA (mint = M) --[amount × 10^9]--> recipient ATA (mint = M)
```

Mint supply is unchanged — only balances move.

**State changes:** sender's balance decreases; recipient's
balance increases; possibly a new ATA created for the recipient.

**Checks enforced by Anchor + SPL Token:**
- `sender_token_account.mint == mint_account`.
- `sender_token_account.owner == sender.key()`.
- `recipient_token_account` is the correct ATA for (mint,
  recipient).
- `sender` signs (authority for their own ATA).
- SPL Token ensures `sender_token_account.amount >= amount` and
  the mint is not frozen (if freeze authority is set).

## 5. Worked example

```
1. Setup (prior instructions):
   - create_token("Joe Coin", "JOE", "...")    -> mint M
   - mint_token(100) signed by Alice, mint=M  -> Alice ATA has 100 JOE

2. Alice calls transfer_tokens(amount = 30), recipient = Bob.
   - Bob's ATA for M doesn't exist yet → Anchor creates it
     (Alice pays ~0.002 SOL rent).
   - token::transfer moves 30 × 10^9 = 30_000_000_000 from Alice
     to Bob's ATA.
   - Balances: Alice 70 JOE, Bob 30 JOE.

3. Alice calls transfer_tokens(amount = 5), recipient = Bob again.
   - ATA exists, init_if_needed skips.
   - Balances: Alice 65 JOE, Bob 35 JOE.
```

## 6. Safety and edge cases

- **Amount overflow.** `amount × 10^decimals` uses `u64`. With
  `decimals = 9`, the largest safe `amount` is ~18.4 billion. Any
  larger and you panic.
- **Wrong-mint ATA.** If a caller passes a malformed
  `recipient_token_account`, Anchor's `associated_token::mint =
  mint_account` constraint rejects it. Still, using
  `transfer_checked` in production is safer because it double-checks.
- **Frozen accounts.** If the mint has a freeze authority and the
  sender's ATA is frozen, the transfer fails.
- **Zero-amount transfer.** Allowed. A no-op. Still pays transaction
  fee.
- **Sender = recipient.** Anchor will still create the "recipient"
  ATA as if it's distinct, even if the sender and recipient are
  the same — but since the ATA is derived from the pubkey, the
  derived address is identical to `sender_token_account`. The
  `init_if_needed` sees it as already initialised and becomes a
  no-op. The transfer to itself is a no-op. Not useful; doesn't
  break anything.
- **ATA rent burden.** The sender pays for new recipient ATAs. For
  payment apps where you don't want the sender to subsidise new
  recipients, have clients pre-create ATAs or accept the (tiny)
  cost.

## 7. Running the tests

```bash
# Anchor
cd anchor && anchor build && anchor test

# Native
cd native && cargo build-sbf && pnpm install && pnpm test

# Quasar
cd quasar && cargo test
```

Tests create a mint, mint tokens to the sender, transfer some to a
recipient, then assert both balances.

## 8. Extending the program

- **Switch to `transfer_checked`.** Pass the mint and decimals into
  the CPI; catches mint-mismatch bugs at runtime.
- **Charge a fee.** Take a protocol fee ATA; split the amount so
  `fee = amount * 100 / 10_000` (1%) goes there and the remainder
  to the recipient.
- **Delegate-backed transfers.** Have the program hold a delegate
  on the sender's ATA (via `token::approve`) and do the transfer
  without the sender signing each time. Useful for automated
  treasury flows.
- **Whitelist recipients.** Maintain a whitelist PDA; reject
  transfers to recipients not on the list.
- **Mint-program CPI.** For Token-2022 mints with a transfer hook,
  you'd add the hook extra accounts + CPI. See
  `tokens/token-extensions/transfer-hook/`.
