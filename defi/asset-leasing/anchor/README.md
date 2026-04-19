# Asset Leasing

A fixed-term SPL-token lease on Solana, with a second-by-second rent
stream, a separate collateral deposit, and a Pyth-oracle-triggered
seizure path when the collateral is no longer worth enough.

This README is a teaching document. If you have never written a Solana
program before and have no background in finance, you are the target
reader — every term that might be unfamiliar is explained the first time
it appears, and every instruction is walked through step by step with
the exact token movements it causes.

If you already know what collateral, a maintenance margin and an oracle
are, you can skip straight to the [Accounts and PDAs](#3-accounts-and-pdas)
or [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
sections.

---

## Table of contents

1. [What does this program do?](#1-what-does-this-program-do)
2. [Glossary](#2-glossary)
3. [Accounts and PDAs](#3-accounts-and-pdas)
4. [Instruction lifecycle walkthrough](#4-instruction-lifecycle-walkthrough)
5. [Full-lifecycle worked examples](#5-full-lifecycle-worked-examples)
6. [Safety and edge cases](#6-safety-and-edge-cases)
7. [Running the tests](#7-running-the-tests)
8. [Extending the program](#8-extending-the-program)

---

## 1. What does this program do?

Two users, a **lessor** and a **lessee**, want to swap SPL tokens
temporarily:

- The lessor has some number of tokens of SPL mint **A** (call it the
  "leased mint") they would like to hand over for a fixed period of
  time.
- The lessee has tokens of a different SPL mint **B** (the "collateral
  mint") they can lock up as a security deposit.

The program acts as a neutral escrow. It:

1. Takes the lessor's A tokens and locks them in a program-owned vault
   until a lessee shows up.
2. When a lessee calls `take_lease`, the program locks the lessee's B
   tokens as collateral and hands the A tokens to the lessee.
3. While the lease is live, a second-by-second **rent stream** pays the
   lessor out of the collateral vault.
4. If the price of A (measured in B) moves against the lessee far enough
   that the locked collateral is no longer enough to cover the cost of
   re-acquiring the leased tokens, anyone can call `liquidate` — the
   collateral is seized, most of it goes to the lessor, a small
   percentage goes to whoever called the liquidation.
5. If the lessee returns the full A amount before the deadline, they get
   back whatever collateral is left after rent.
6. If the lessee ghosts past the deadline without returning anything,
   the lessor calls `close_expired` and sweeps the collateral as
   compensation.

Nothing mysterious: the program is a pair of vaults, a small piece of
state that tracks how much rent has been paid, and an oracle check. It
is written in Anchor.

### The tradfi picture, briefly

For readers who have never encountered a real-world leasing or margin
arrangement — two quick analogies. They are strictly optional; the
program is fully described above in Solana terms.

- **Think of hiring a car.** You pay the rental firm a refundable
  deposit and a daily fee. If you return the car on time and in one
  piece, you get the deposit back. If you drive off and disappear, they
  keep the deposit. Here the lessor is the rental firm, the lessee is
  you, the leased tokens are the car, and the collateral is the
  deposit.

- **Think of a pawn shop loan.** You hand over something valuable
  (collateral), you borrow something in return. If the value of what
  you handed over drops — for example, if you pawned gold and the gold
  price collapsed — the shop can sell your collateral before it's worth
  less than they lent you. On Solana, a price oracle tells the program
  when that moment has arrived, and `liquidate` does the selling.

Neither analogy is exact (a car rental doesn't usually charge rent in
the same asset it took as a deposit, a pawn shop doesn't usually set a
hard deadline). The onchain mechanics are what matters below.

### What this example is not

- **It is not a deployed, audited production program.** Treat it as a
  learning example. It makes simplifying choices (see §6) that a
  production lease protocol would need to revisit.
- **It does not pretend to match mainnet Pyth behaviour exactly.** The
  LiteSVM tests install a hand-rolled `PriceUpdateV2` account; on
  mainnet you would use the real Pyth Receiver crate.

---

## 2. Glossary

Terms appearing anywhere below, explained in terms of what they are
mechanically.

**Account**
: On Solana, every piece of state — a user wallet, a token balance, a
program's config — is an *account*. An account has an address (a
32-byte public key), a length, some lamports holding it rent-exempt, an
owner program (the only program that can mutate the bytes), and a
byte buffer (`data`).

**Lamport**
: The smallest unit of SOL. 1 SOL = 10⁹ lamports. Accounts must hold
enough lamports to be "rent-exempt" for their size; the program
reimburses these lamports when it closes an account.

**Signer**
: An account whose private key signed the transaction. Only signers can
authorise transfers out of accounts they own (including normal
wallets). The list of signers is attached to every transaction.

**SPL token**
: Solana's equivalent of an ERC-20. An SPL *mint* account describes
the token (its supply, decimals, authority). Each user's balance of a
given mint lives in a separate *token account* owned by the SPL Token
program.

**Token account**
: An account that holds a balance of a specific SPL mint, controlled by
an *authority* (usually a user's wallet pubkey, but can be a PDA). In
this program, the two vaults are token accounts whose authority is the
vault PDA itself.

**Associated Token Account (ATA)**
: A conventional, deterministic token account address for a given
`(wallet, mint)` pair. Derived by the SPL Associated Token Account
program. When you send USDC to "someone's wallet", you really mean
their ATA for the USDC mint. The program creates lessor/lessee ATAs on
demand (`init_if_needed`) so callers don't have to pre-create them.

**PDA (Program Derived Address)**
: A deterministic address derived from a list of "seeds" plus a
program id, via `Pubkey::find_program_address`. PDAs have no private
key. A program can *sign* as a PDA in a CPI by producing the seeds —
that's the only way to move tokens out of a PDA-owned vault. In this
program there are three PDAs per lease: the `Lease` state account, the
`leased_vault` token account, and the `collateral_vault` token
account.

**Seeds**
: The byte strings that, together with the program id, deterministically
derive a PDA. For this program the seeds are `[b"lease", lessor,
lease_id]` for the state account and `[b"leased_vault", lease]` /
`[b"collateral_vault", lease]` for the vaults.

**Bump**
: A one-byte offset that, together with the seeds, produces an address
that is *not* on the Ed25519 curve (i.e. has no corresponding private
key). `find_program_address` finds the highest bump that yields an
off-curve address. Stored on the `Lease` account so the program doesn't
have to recompute it every time it signs.

**CPI (Cross-Program Invocation)**
: One program calling another within the same transaction. The SPL
Token program's `TransferChecked` and `CloseAccount` instructions are
the CPIs used here.

**Anchor**
: A Rust framework for writing Solana programs. The `#[derive(Accounts)]`
macro generates the account-validation boilerplate — ownership checks,
signer checks, PDA derivation, constraint checks like `has_one` — from
struct definitions. The `#[account]` macro handles serialising program
state accounts with an 8-byte discriminator prefix so the program can
tell different account types apart.

**Anchor constraint**
: An attribute on an account field in a `#[derive(Accounts)]` struct,
like `mut`, `seeds = [...]`, `has_one = lessor`, or
`constraint = lease.status == LeaseStatus::Active`. Each one expands
into a check that runs before the handler executes. If any check fails
the transaction is rejected.

**Discriminator**
: The first 8 bytes of an Anchor account, equal to the first 8 bytes of
`sha256("account:<StructName>")`. Anchor writes them at initialisation
and checks them on every deserialisation so one struct's bytes cannot
be mistaken for another's.

**Rent (Solana)**
: The lamports deposit that keeps an account alive. Since it's always
paid up-front (rent-exempt), you can think of it as a refundable
security deposit from a payer. When an account is closed the lamports
are returned to whichever account is specified as `close = ...` in
Anchor.

**Rent (this program)**
: The per-second payment the lessee owes the lessor for holding the
leased tokens. Measured in collateral-mint base units, streams from
the collateral vault to the lessor's collateral ATA on every
`pay_rent`. *Unrelated to Solana account rent* — same word, different
meaning. Context usually makes it obvious.

**Vault**
: In this codebase, one of the two program-owned token accounts (leased
or collateral). Their authority is the PDA itself, so the program is
the only thing that can move funds out of them, and it does so by
producing the vault's PDA seeds when making the transfer CPI.

**Basis point (bps)**
: 1/100 of a percent. 10 000 bps = 100%. Used here for the maintenance
margin and liquidation bounty. Integer-only bps arithmetic keeps all
percentage calculations free of floating-point error.

**Maintenance margin**
: A ratio. The liquidation check asks: is the collateral's value (in
collateral-mint units) at least `maintenance_margin_bps / 10_000`
times the debt's value (the leased amount, priced into the same
units)? For `maintenance_margin_bps = 12_000` that is 120%. Drop below
and the position is liquidatable. This is the "how much cushion must
the lessee keep on top of the raw value of the leased asset".

**Liquidation**
: The instruction (`liquidate`) that closes an underwater lease. Rent
is first paid from the collateral vault; then a percentage (the
*liquidation bounty*) of whatever collateral is left goes to the
keeper who called the instruction, and the remainder goes to the
lessor. Lease status becomes `Liquidated`.

**Keeper**
: Any party — usually a bot — that calls a permissionless instruction
to keep the protocol healthy. Here the keeper calls `liquidate` when
they spot an underwater lease. They are paid the `liquidation_bounty`
for their trouble.

**Oracle**
: An onchain account whose bytes are periodically updated with
information from the outside world — for this program, the current
price of the leased mint priced in units of the collateral mint. We
use Pyth's `PriceUpdateV2` accounts.

**Pyth `PriceUpdateV2`**
: The Pyth receiver program owns a set of accounts, each with a fixed
layout: discriminator (8) + write_authority (32) + verification_level
(1) + `feed_id` (32) + price (i64, 8) + conf (u64, 8) + exponent
(i32, 4) + publish_time (i64, 8) + …. This program only reads
`feed_id`, `price`, `exponent` and `publish_time`.

**Feed id**
: A 32-byte identifier for a specific Pyth price feed (e.g.
"BONK/USD"). Pinned on the `Lease` at creation so a keeper cannot swap
in a different feed during a liquidation call to force an underwater
verdict.

**Exponent**
: Pyth prices are integer pairs `(price, exponent)`; the real price is
`price * 10^exponent`. For example `(12345, -2)` means 123.45. All of
this program's math is integer and folds the exponent into whichever
side of the inequality doesn't already have the denominator applied.

---

## 3. Accounts and PDAs

Every call to the program touches some subset of these accounts. The
three PDAs are created on `create_lease` and destroyed on `return_lease`
/ `liquidate` / `close_expired`.

### State / data accounts

| Account | PDA? | Seeds | Kind | Authority | Holds |
|---|---|---|---|---|---|
| `Lease` | yes | `["lease", lessor, lease_id]` | data | program | all the lease parameters and current lifecycle state (see below) |

### Token vaults

| Account | PDA? | Seeds | Kind | Authority | Holds |
|---|---|---|---|---|---|
| `leased_vault` | yes | `["leased_vault", lease]` | SPL token account | itself (PDA-signed) | `leased_amount` while `Listed`; 0 while `Active` (lessee has the tokens); full amount again briefly inside `return_lease` |
| `collateral_vault` | yes | `["collateral_vault", lease]` | SPL token account | itself (PDA-signed) | 0 while `Listed`; `collateral_amount` while `Active`, decreasing as rent streams out and increasing on `top_up_collateral` |

### User accounts passed in

| Account | Owner | Purpose |
|---|---|---|
| `lessor` wallet | user | `create_lease` signer, receives rent and final recovery |
| `lessee` wallet | user | `take_lease` / `top_up_collateral` / `return_lease` signer |
| `keeper` wallet | user | `liquidate` signer, receives the bounty |
| `payer` wallet | user | `pay_rent` signer (can be anyone, not just the lessee) |
| `lessor_leased_account` | SPL Token | lessor's ATA for the leased mint; source on `create_lease`, destination on `return_lease` / `close_expired` |
| `lessor_collateral_account` | SPL Token | lessor's ATA for the collateral mint; destination for rent and liquidation proceeds |
| `lessee_leased_account` | SPL Token | lessee's ATA for the leased mint; destination on `take_lease`, source on `return_lease` |
| `lessee_collateral_account` | SPL Token | lessee's ATA for the collateral mint; source on `take_lease` / `top_up_collateral`, destination for collateral refund on `return_lease` |
| `keeper_collateral_account` | SPL Token | keeper's ATA for the collateral mint; receives the liquidation bounty |
| `price_update` | Pyth Receiver program | `PriceUpdateV2` account for the feed the lease is pinned to |

### Fields on `Lease`

From [`state/lease.rs`](programs/asset-leasing/src/state/lease.rs):

```rust
pub struct Lease {
    pub lease_id: u64,             // caller-supplied id so one lessor can run many leases
    pub lessor: Pubkey,            // who listed it, gets paid rent
    pub lessee: Pubkey,            // who took it; Pubkey::default() while Listed

    pub leased_mint: Pubkey,
    pub leased_amount: u64,        // locked at creation, unchanging

    pub collateral_mint: Pubkey,
    pub collateral_amount: u64,    // increases on top_up, decreases as rent pays out
    pub required_collateral_amount: u64, // what the lessee must post on take_lease

    pub rent_per_second: u64,      // denominated in collateral units
    pub duration_seconds: i64,
    pub start_ts: i64,             // 0 while Listed
    pub end_ts: i64,               // 0 while Listed; start_ts + duration once Active
    pub last_rent_paid_ts: i64,    // rent accrues from here to min(now, end_ts)

    pub maintenance_margin_bps: u16,   // e.g. 12_000 = 120%
    pub liquidation_bounty_bps: u16,   // e.g. 500 = 5%

    pub feed_id: [u8; 32],         // Pyth feed_id this lease is pinned to

    pub status: LeaseStatus,       // Listed | Active | Liquidated | Closed

    pub bump: u8,
    pub leased_vault_bump: u8,
    pub collateral_vault_bump: u8,
}
```

### Lifecycle diagram

```
                  create_lease
               +---------------+
 (no lease) -> |    Listed     |
               +---------------+
                 |          |
      take_lease |          | close_expired (lessor cancels)
                 v          v
               +---------------+       +--------+
               |    Active     | ----> | Closed |
               +---------------+       +--------+
                 |    |       |
     return_lease|    |       | close_expired (after end_ts)
                 |    | liquidate
                 v    v       v
             +--------+ +-----------+
             | Closed | | Liquidated|
             +--------+ +-----------+
```

The `Closed` and `Liquidated` states are not directly observable
onchain: all three of `return_lease`, `liquidate` and `close_expired`
close the `Lease` account in the same instruction (`close = lessor`),
returning the rent-exempt lamports to the lessor. The in-memory
`status` field is set *before* the close so the transaction logs
record the terminal state, but the account disappears at the end.

---

## 4. Instruction lifecycle walkthrough

The program has seven instructions. The natural order a user encounters
them — the order below — is:

1. `create_lease` (lessor)
2. `take_lease` (lessee)
3. `pay_rent` (anyone)
4. `top_up_collateral` (lessee)
5. `return_lease` (lessee) — **happy path**
6. `liquidate` (keeper) — **adversarial path**
7. `close_expired` (lessor) — **default / cancel path**

For each, the shape is the same: who signs, what accounts go in, which
PDAs get created or closed, which tokens move, what state changes, what
checks the program runs.

Token-flow diagrams use the following shorthand:

```
  <source account> --[amount of <mint>]--> <destination account>
```

### 4.1 `create_lease`

**Who calls it:** the lessor. They want to offer some number of leased
tokens for a fixed term against collateral of a different mint.

**Signers:** `lessor`.

**Parameters:**

```rust
pub fn create_lease(
    context: Context<CreateLease>,
    lease_id: u64,
    leased_amount: u64,
    required_collateral_amount: u64,
    rent_per_second: u64,
    duration_seconds: i64,
    maintenance_margin_bps: u16,
    liquidation_bounty_bps: u16,
    feed_id: [u8; 32],
) -> Result<()>
```

**Accounts in:**

- `lessor` (signer, mut — pays account rent)
- `leased_mint`, `collateral_mint` (read-only)
- `lessor_leased_account` (mut, lessor's ATA for the leased mint — source)
- `lease` (PDA, **init**) — created here
- `leased_vault` (PDA, **init**, token account) — created here
- `collateral_vault` (PDA, **init**, token account) — created here
- `token_program`, `system_program`

**PDAs created:**

- `lease` with seeds `[b"lease", lessor, lease_id.to_le_bytes()]`
- `leased_vault` with seeds `[b"leased_vault", lease]`, authority = itself
- `collateral_vault` with seeds `[b"collateral_vault", lease]`, authority = itself

**Checks (from `handle_create_lease`):**

- `leased_mint != collateral_mint` → `LeasedMintEqualsCollateralMint`
- `leased_amount > 0` → `InvalidLeasedAmount`
- `required_collateral_amount > 0` → `InvalidCollateralAmount`
- `rent_per_second > 0` → `InvalidRentPerSecond`
- `duration_seconds > 0` → `InvalidDuration`
- `0 < maintenance_margin_bps <= 50_000` → `InvalidMaintenanceMargin`
- `liquidation_bounty_bps <= 2_000` → `InvalidLiquidationBounty`

**Token movements:**

```
  lessor_leased_account --[leased_amount of leased_mint]--> leased_vault PDA
```

**State changes:**

- New `Lease` account written with `status = Listed`, `lessee =
  Pubkey::default()`, `collateral_amount = 0`, `start_ts = 0`,
  `end_ts = 0`, `last_rent_paid_ts = 0`, and the given parameters
  including `feed_id`. All three bumps stored.

**Why lock the leased tokens up-front rather than on `take_lease`?** So a
lessee who calls `take_lease` cannot possibly fail because the lessor
doesn't have the tokens any more — the atomicity guarantee is
transferred to the PDA the moment the lease is listed.

### 4.2 `take_lease`

**Who calls it:** the lessee. They have seen the `Lease` account on
chain (somehow — an indexer, a direct lookup, whatever) and want to
take delivery.

**Signers:** `lessee`.

**Accounts in:**

- `lessee` (signer, mut)
- `lessor` (UncheckedAccount — read for PDA seed derivation only, no
  signature required)
- `lease` (mut, `has_one = lessor`, `has_one = leased_mint`,
  `has_one = collateral_mint`, must be `Listed`)
- `leased_mint`, `collateral_mint`
- `leased_vault`, `collateral_vault` (both mut, both PDA-derived)
- `lessee_collateral_account` (mut, lessee's ATA — source)
- `lessee_leased_account` (mut, **init_if_needed** — destination)
- `token_program`, `associated_token_program`, `system_program`

**Checks:**

- `lease.status == Listed` → `InvalidLeaseStatus`
- `lease.lessor == lessor.key()` (Anchor `has_one`)
- `lease.leased_mint == leased_mint.key()` (Anchor `has_one`)
- `lease.collateral_mint == collateral_mint.key()` (Anchor `has_one`)

**Token movements (in order):**

```
  lessee_collateral_account --[required_collateral_amount of collateral_mint]--> collateral_vault PDA
  leased_vault PDA         --[leased_amount of leased_mint]-----------------> lessee_leased_account
```

Collateral is deposited *first* so if the leased-token transfer fails
for any reason the whole transaction reverts and the lessee gets their
collateral back.

**State changes:**

- `lease.lessee = lessee.key()`
- `lease.collateral_amount = required_collateral_amount`
- `lease.start_ts = now`
- `lease.end_ts = now + duration_seconds` (checked add, errors on overflow)
- `lease.last_rent_paid_ts = now` (nothing has accrued yet)
- `lease.status = Active`

### 4.3 `pay_rent`

**Who calls it:** anyone. The lessee's incentive is obvious (keep the
lease from going underwater); a keeper bot may also push rent before a
liquidation check so healthy leases stay healthy.

**Signers:** `payer` (any signer).

**Accounts in:**

- `payer` (signer, mut — pays for `init_if_needed` of the lessor ATA)
- `lessor` (UncheckedAccount, read-only — used for `has_one` check)
- `lease` (mut, must be `Active`)
- `collateral_mint`, `collateral_vault`
- `lessor_collateral_account` (mut, **init_if_needed**)
- `token_program`, `associated_token_program`, `system_program`

**Rent math:**

```rust
pub fn compute_rent_due(lease: &Lease, now: i64) -> Result<u64> {
    let cutoff = now.min(lease.end_ts);
    if cutoff <= lease.last_rent_paid_ts {
        return Ok(0);
    }
    let elapsed = (cutoff - lease.last_rent_paid_ts) as u64;
    elapsed.checked_mul(lease.rent_per_second)
        .ok_or(AssetLeasingError::MathOverflow.into())
}
```

Rent does not accrue past `end_ts`. Past the deadline the lessee is
either returning the tokens (via `return_lease`), being liquidated, or
defaulting — no more rent is owed.

**Token movements:**

```
  collateral_vault PDA --[min(rent_due, collateral_amount) of collateral_mint]--> lessor_collateral_account
```

If the vault does not have enough collateral to cover the full
`rent_due`, the handler pays out whatever is there and leaves the
residual as a debt the next liquidation (or `close_expired`) will
clean up.

**State changes:**

- `lease.collateral_amount -= payable`
- `lease.last_rent_paid_ts = now.min(end_ts)`

### 4.4 `top_up_collateral`

**Who calls it:** the lessee — to defend against a looming liquidation
by adding more of the collateral mint to the vault.

**Signers:** `lessee`.

**Accounts in:**

- `lessee` (signer)
- `lessor` (UncheckedAccount, read-only)
- `lease` (mut, `has_one = lessor`, `has_one = collateral_mint`,
  `constraint lease.lessee == lessee.key()`, must be `Active`)
- `collateral_mint`, `collateral_vault`
- `lessee_collateral_account` (mut, source)
- `token_program`

**Parameter:** `amount: u64` — how much to add.

**Checks:**

- `amount > 0` → `InvalidCollateralAmount`
- `lease.lessee == lessee.key()` → `Unauthorised`
- `lease.status == Active` → `InvalidLeaseStatus`

**Token movements:**

```
  lessee_collateral_account --[amount of collateral_mint]--> collateral_vault PDA
```

**State changes:**

- `lease.collateral_amount += amount` (checked add)

### 4.5 `return_lease`

**Who calls it:** the lessee, while the lease is still `Active` and
before or after `end_ts` (the only timing rule is that `status ==
Active`; rent only accrues up to `end_ts` so returning after the
deadline does not pile on extra charges).

**Signers:** `lessee`.

**Accounts in:**

- `lessee` (signer, mut)
- `lessor` (UncheckedAccount, mut — receives Lease and vault rent-exempt
  lamports via `close = lessor`)
- `lease` (mut, `close = lessor`, must be `Active`, `lessee == lessee.key()`)
- `leased_mint`, `collateral_mint`
- `leased_vault`, `collateral_vault` (both mut)
- `lessee_leased_account` (mut, source for the return)
- `lessee_collateral_account` (mut, destination for the refund)
- `lessor_leased_account` (mut, **init_if_needed**)
- `lessor_collateral_account` (mut, **init_if_needed**)
- `token_program`, `associated_token_program`, `system_program`

**Checks:**

- `lease.status == Active` → `InvalidLeaseStatus`
- `lease.lessee == lessee.key()` → `Unauthorised`

**Token movements (in order):**

```
  lessee_leased_account   --[leased_amount of leased_mint]----------> leased_vault PDA
  leased_vault PDA        --[leased_amount of leased_mint]----------> lessor_leased_account
  collateral_vault PDA    --[rent_payable of collateral_mint]-------> lessor_collateral_account
  collateral_vault PDA    --[collateral_after_rent of collateral_mint]--> lessee_collateral_account
```

The leased tokens hop through the vault rather than going direct
lessee→lessor because the vault's token account is already set up and
the program can reuse its PDA signing path. The atomic round-trip keeps
the vault's post-ix balance at 0 so it can be closed.

After the transfers:

- Both vaults are closed via `close_account` CPIs; their rent-exempt
  lamports go to the lessor.
- The `Lease` account is closed via Anchor's `close = lessor`
  constraint; its rent-exempt lamports go to the lessor too.

**State changes before close:**

- `lease.last_rent_paid_ts = now.min(end_ts)`
- `lease.collateral_amount = 0`
- `lease.status = Closed`

### 4.6 `liquidate`

**Who calls it:** a keeper, when they can prove the position is
underwater.

**Signers:** `keeper`.

**Accounts in:**

- `keeper` (signer, mut — pays `init_if_needed` cost for both ATAs)
- `lessor` (UncheckedAccount, mut — receives rent + lessor_share + the
  `Lease` and vault rent-exempt lamports)
- `lease` (mut, `close = lessor`, must be `Active`)
- `leased_mint`, `collateral_mint`
- `leased_vault`, `collateral_vault` (both mut)
- `lessor_collateral_account` (mut, **init_if_needed**)
- `keeper_collateral_account` (mut, **init_if_needed**)
- `price_update` (UncheckedAccount, constrained to `owner =
  PYTH_RECEIVER_PROGRAM_ID`)
- `token_program`, `associated_token_program`, `system_program`

**Checks (in order, early-out on failure):**

1. `price_update.owner == Pyth Receiver program id` (Anchor `owner =`)
2. Account data decodes as `PriceUpdateV2` (first 8 bytes match
   `PRICE_UPDATE_V2_DISCRIMINATOR`; length ≥ 89 bytes) — else
   `StalePrice`
3. `decoded.feed_id == lease.feed_id` → `PriceFeedMismatch`
4. `publish_time <= now` (no future stamps) and
   `now - publish_time <= 60 seconds` → `StalePrice`
5. `price > 0` → `NonPositivePrice`
6. `is_underwater(lease, price, now) == true` → `PositionHealthy`
7. `lease.status == Active` (Anchor constraint on the `lease` field)

The underwater check, in integers:

```
  collateral_value_in_colla_units * 10_000
      <  debt_value_in_colla_units * maintenance_margin_bps
```

where `debt_value = leased_amount * price * 10^exponent` (with the
exponent folded into whichever side keeps the math non-negative, see
[`is_underwater`](programs/asset-leasing/src/instructions/liquidate.rs)).

**Token movements:**

```
  collateral_vault PDA --[rent_payable of collateral_mint]---------------------> lessor_collateral_account
  collateral_vault PDA --[bounty = remaining * bounty_bps / 10_000]-----------> keeper_collateral_account
  collateral_vault PDA --[remaining - bounty of collateral_mint]--------------> lessor_collateral_account
  leased_vault PDA    --[0 of leased_mint]  (empty — lessee kept the tokens)    close only
```

After the three outbound collateral transfers (rent, bounty, lessor
share) the collateral_vault is empty. Both vaults are then closed —
their rent-exempt lamports go to the lessor. The `Lease` account is
closed the same way (Anchor `close = lessor`).

**State changes before close:**

- `lease.collateral_amount = 0`
- `lease.last_rent_paid_ts = now.min(end_ts)`
- `lease.status = Liquidated`

### 4.7 `close_expired`

**Who calls it:** the lessor. Two very different situations collapse
into this single instruction:

- **Cancel a `Listed` lease** — the lessor changes their mind, no-one
  has taken the lease yet. Allowed any time.
- **Reclaim collateral after default** — the lease is `Active`, `now >=
  end_ts`, the lessee has not called `return_lease`. The lessor takes
  the whole collateral vault as compensation.

**Signers:** `lessor`.

**Accounts in:**

- `lessor` (signer, mut — also the rent destination for all three closes)
- `lease` (mut, `close = lessor`, status ∈ `{Listed, Active}`)
- `leased_mint`, `collateral_mint`
- `leased_vault`, `collateral_vault` (both mut)
- `lessor_leased_account` (mut, **init_if_needed**)
- `lessor_collateral_account` (mut, **init_if_needed**)
- `token_program`, `associated_token_program`, `system_program`

**Checks:**

- `status ∈ {Listed, Active}` (Anchor `constraint matches!(...)`) →
  `InvalidLeaseStatus`
- If `status == Active`, also `now >= end_ts` → `LeaseNotExpired`

**Token movements:**

For a `Listed` cancel:
```
  leased_vault PDA --[leased_amount of leased_mint]--> lessor_leased_account
  collateral_vault PDA is empty (0 transferred)
```

For an `Active` default:
```
  leased_vault PDA is empty (lessee kept the tokens)
  collateral_vault PDA --[collateral_amount of collateral_mint]--> lessor_collateral_account
```

In both cases both vaults are then closed and the `Lease` account is
closed; all three rent-exempt lamport refunds go to the lessor.

**State changes before close:**

- If `Active`: `lease.last_rent_paid_ts = now.min(end_ts)`
  (settles the accounting so any future program version that wants
  to split the default pot differently has a correct timestamp to
  start from)
- `lease.collateral_amount = 0`
- `lease.status = Closed`

---

## 5. Full-lifecycle worked examples

All three use the same starting numbers so the arithmetic is easy to
follow. Both mints are 6-decimal SPL tokens. "LEASED" means one base
unit of the leased mint; "COLLA" means one base unit of the collateral
mint.

- `leased_amount = 100_000_000` LEASED (100 tokens).
- `required_collateral_amount = 200_000_000` COLLA (200 tokens).
- `rent_per_second = 10` COLLA.
- `duration_seconds = 86_400` (24 hours).
- `maintenance_margin_bps = 12_000` (120%).
- `liquidation_bounty_bps = 500` (5% of post-rent collateral).
- `feed_id = [0xAB; 32]` (arbitrary, consistent across all calls).

Lessor starts with 1 000 000 000 LEASED in their ATA. Lessee starts
with 1 000 000 000 COLLA in theirs.

### 5.1 Happy path — lessee returns on time

Calls, in order:

1. **`create_lease`** — lessor posts 100 LEASED into `leased_vault`,
   parameters written to `lease`.
   ```
   lessor_leased_account --[100_000_000 LEASED]--> leased_vault PDA
   ```
   Balances after: lessor has 900 000 000 LEASED, `leased_vault` has
   100 000 000 LEASED, `collateral_vault` has 0.

2. **`take_lease`** — lessee posts 200 COLLA, receives 100 LEASED.
   ```
   lessee_collateral_account --[200_000_000 COLLA]--> collateral_vault PDA
   leased_vault PDA          --[100_000_000 LEASED]--> lessee_leased_account
   ```
   `lease.status = Active`, `start_ts = T`, `end_ts = T + 86_400`.

3. **`pay_rent`** called at `T + 120` seconds. Rent due = 120 × 10 =
   1 200 COLLA.
   ```
   collateral_vault PDA --[1_200 COLLA]--> lessor_collateral_account
   ```
   `collateral_amount = 200_000_000 − 1_200 = 199_998_800`.

4. **`top_up_collateral(amount = 50_000_000)`** at `T + 600`. Lessee
   decides to add a cushion.
   ```
   lessee_collateral_account --[50_000_000 COLLA]--> collateral_vault PDA
   ```
   `collateral_amount = 199_998_800 + 50_000_000 = 249_998_800`.

5. **`return_lease`** called at `T + 3_600` (one hour in). Total rent
   from `start_ts` to `now` is 3 600 × 10 = 36 000 COLLA; 1 200 of that
   was paid in step 3. Residual rent = 36 000 − 1 200 = 34 800 COLLA.
   ```
   lessee_leased_account  --[100_000_000 LEASED]--> leased_vault PDA
   leased_vault PDA       --[100_000_000 LEASED]--> lessor_leased_account
   collateral_vault PDA   --[34_800 COLLA]--------> lessor_collateral_account
   collateral_vault PDA   --[249_964_000 COLLA]---> lessee_collateral_account
   ```
   Where `249_964_000 = 249_998_800 − 34_800`.

   Both vaults close, their rent-exempt lamports go to the lessor. The
   `Lease` account closes via `close = lessor`.

**Final balances:**

- Lessor: 1 000 000 000 LEASED (full return), 36 000 COLLA (total rent
  received in steps 3 + 5), plus the lamports from three account closes.
- Lessee: 100 000 000 LEASED → 0 (all returned), COLLA: started with
  1 000 000 000, spent 200 000 000 on initial deposit + 50 000 000 on
  top-up, got back 249 964 000, so holds 999 964 000 COLLA (net cost
  of 36 000 — exactly the total rent paid).

### 5.2 Liquidation path

Same setup. Steps 1 and 2 run identically.

3. Time jumps to `T + 300`. A keeper observes a new Pyth price update:
   the leased-in-collateral price has spiked to 4.0 (exponent 0, price
   = 4). At that price, the debt value is `100_000_000 × 4 =
   400_000_000` COLLA. The collateral is still ~`200_000_000` COLLA
   (minus some streamed rent). Maintenance ratio = `200/400 = 50%`,
   well below the required 120%.

   The keeper calls `pay_rent` first is *not* required — `liquidate`
   settles accrued rent itself. It goes straight to `liquidate`.

4. **`liquidate`** at `T + 300`:
   - Rent due = 300 × 10 = 3 000 COLLA; collateral_amount = 200 000 000
     so `rent_payable = 3 000`.
     ```
     collateral_vault PDA --[3_000 COLLA]--> lessor_collateral_account
     ```
   - Remaining = 200 000 000 − 3 000 = 199 997 000 COLLA.
   - Bounty = 199 997 000 × 500 / 10 000 = 9 999 850 COLLA.
     ```
     collateral_vault PDA --[9_999_850 COLLA]--> keeper_collateral_account
     ```
   - Lessor share = 199 997 000 − 9 999 850 = 189 997 150 COLLA.
     ```
     collateral_vault PDA --[189_997_150 COLLA]--> lessor_collateral_account
     ```
   - Both vaults close; Lease closes. Status recorded as `Liquidated`.

**Final balances:**

- Lessor: 900 000 000 LEASED (never got the 100 back — the lessee kept
  them), `3 000 + 189 997 150 = 190 000 150` COLLA, plus rent-exempt
  lamports from three closes.
- Lessee: *still* has 100 000 000 LEASED. Spent 200 000 000 COLLA on
  deposit, got nothing back. Net: they walk away with the leased tokens
  but forfeited the entire collateral minus the keeper's cut.
- Keeper: 9 999 850 COLLA for their trouble.

(This is the key asymmetry: liquidation does *not* reclaim the leased
tokens. The collateral pays the lessor for the lost asset. The lessee
has effectively bought the leased tokens at the forfeit price.)

### 5.3 Default / expiry path — `close_expired` on an `Active` lease

Same setup. Steps 1 and 2 run as usual. The lessee takes the tokens,
posts collateral, then disappears.

3. `pay_rent` is never called. Clock advances all the way past
   `end_ts = T + 86_400`.

4. **`close_expired`** called by the lessor at `T + 100_000`:
   - `status == Active` and `now >= end_ts` → the default branch runs.
   - `leased_vault` is empty (lessee kept the tokens). No transfer.
   - `collateral_vault` has 200 000 000 COLLA. All of it goes to the
     lessor:
     ```
     collateral_vault PDA --[200_000_000 COLLA]--> lessor_collateral_account
     ```
   - Both vaults close; Lease closes.
   - `last_rent_paid_ts = min(now, end_ts) = end_ts` (step added in
     Fix 5).

**Final balances:**

- Lessor: 900 000 000 LEASED, 200 000 000 COLLA (the whole collateral
  as compensation), plus three account-close refunds.
- Lessee: 100 000 000 LEASED, −200 000 000 COLLA. They paid the whole
  collateral and kept the leased tokens.

### 5.4 Default / expiry path — `close_expired` on a `Listed` lease

This is the cheap cancel path. No lessee ever showed up.

1. `create_lease` as above.
2. `close_expired` called by the lessor immediately.
   - `status == Listed` → no expiry check.
   - `leased_vault` holds 100 000 000 LEASED. Drain back:
     ```
     leased_vault PDA --[100_000_000 LEASED]--> lessor_leased_account
     ```
   - `collateral_vault` is empty. No transfer.
   - Both vaults close; Lease closes.

**Final balances:** lessor is back to 1 000 000 000 LEASED; nothing
else moved.

---

## 6. Safety and edge cases

### 6.1 What the program refuses to do

All of the following come from [`errors.rs`](programs/asset-leasing/src/errors.rs)
and are enforced by either an Anchor constraint or a `require!` in the
handler:

| Error | When |
|---|---|
| `InvalidLeaseStatus` | Action tried against a lease in the wrong state (e.g. `take_lease` on a lease that is already `Active`) |
| `InvalidDuration` | `duration_seconds <= 0` on `create_lease` |
| `InvalidLeasedAmount` | `leased_amount == 0` on `create_lease` |
| `InvalidCollateralAmount` | `required_collateral_amount == 0` on `create_lease`; `amount == 0` on `top_up_collateral` |
| `InvalidRentPerSecond` | `rent_per_second == 0` on `create_lease` |
| `InvalidMaintenanceMargin` | `maintenance_margin_bps == 0` or `> 50_000` on `create_lease` |
| `InvalidLiquidationBounty` | `liquidation_bounty_bps > 2_000` on `create_lease` |
| `LeaseExpired` | Reserved; not currently used (rent accrual naturally caps at `end_ts`) |
| `LeaseNotExpired` | `close_expired` called on an `Active` lease before `end_ts` |
| `PositionHealthy` | `liquidate` called on a lease that passes the maintenance-margin check |
| `StalePrice` | Pyth price update older than 60 s, or has a future `publish_time`, or fails discriminator / length check |
| `NonPositivePrice` | Pyth price is `<= 0` |
| `MathOverflow` | Any of the `checked_*` arithmetic returned `None` |
| `Unauthorised` | Lease-modifying instruction called by someone who is not the registered lessee (`top_up_collateral`, `return_lease`) |
| `LeasedMintEqualsCollateralMint` | `create_lease` called with the same mint for both sides |
| `PriceFeedMismatch` | `liquidate` called with a Pyth update whose `feed_id` does not match `lease.feed_id` |

### 6.2 Guarded design choices worth knowing

- **Leased tokens are locked up-front.** `create_lease` moves the tokens
  into the `leased_vault` immediately, so a lessee calling `take_lease`
  cannot fail because the lessor spent the funds elsewhere in the
  meantime.

- **Leased mint ≠ collateral mint.** If both sides used the same SPL
  mint, the two vaults would hold the same asset and the
  "what-do-I-owe-vs-what-do-I-hold" accounting would collapse. The
  guard is cheap and the error message is explicit.

- **Feed pinning.** The Pyth `feed_id` is stored on the `Lease` at
  creation and enforced on every `liquidate`. A keeper cannot pass in a
  random unrelated price feed (like a volatile pair that happens to be
  dipping) to force a spurious liquidation.

- **Staleness window.** Pyth `publish_time` older than 60 seconds is
  rejected, and `publish_time > now` is rejected too (keepers must not
  front-run the validator clock).

- **Integer-only math.** Every percentage and price calculation folds
  into a `checked_mul` / `checked_div` of `u128` — no floats, no
  surprising NaN. `BPS_DENOMINATOR = 10 000` is the only
  "percentage denominator" anywhere; cross-check against `constants.rs`
  if you're porting the math.

- **Authority-is-self vaults.** `leased_vault.authority ==
  leased_vault.key()` (and likewise for `collateral_vault`). The
  program signs as the vault using its own seeds, which means the
  `Lease` account is not involved in signing any of the token moves.
  This keeps the signer-seed array small (one seed list, not two).

- **Max maintenance margin = 500%.** Without an upper bound a lessor
  could set a margin that is unreachable on day one and liquidate the
  lessee instantly. 50 000 bps is generous — enough for truly
  speculative leases — while still blocking the pathological 10 000×
  trap.

- **Max liquidation bounty = 20%.** Higher than 20% and the keeper's
  cut would dwarf the lessor's recovery on default. The cap keeps
  liquidation economics roughly in line with lender-first semantics.

### 6.3 Things the program does *not* guard against

A production lease protocol would want more, but this is an example:

- **Price feed correctness.** The program verifies the owner
  (`PYTH_RECEIVER_PROGRAM_ID`), the discriminator, the layout and the
  feed id, but it cannot know whether the feed the lessor pinned
  quotes the right pair. Supplying the wrong feed at creation is the
  lessor's problem — it won't cause a liquidation to succeed against a
  truly healthy position (the feed id check would fail), but it will
  mean *no* liquidation can succeed, so a lessee could drain the
  collateral via rent and walk away. A production version would cross-
  check the price feed's `feed_id` against a protocol registry.

- **Rent dust accumulation.** Rent is paid in whole base units per
  second of `rent_per_second`. Choose a small `rent_per_second` and
  short-lived leases can settle 0 rent if no-one calls `pay_rent` for
  a very short period. Not a security issue — the accrual ts only
  moves forward when rent is actually settled — but worth knowing.

- **Griefing on `init_if_needed`.** `take_lease`, `pay_rent`,
  `liquidate`, `return_lease` and `close_expired` all do
  `init_if_needed` on one or more ATAs. If the caller does not fund
  the rent-exempt reserve for those accounts, the transaction fails.
  This is the intended behaviour (the caller pays for the state they
  require) but can surprise a lessee on a tight SOL budget.

- **No partial rent refund on default.** When `close_expired` runs on
  an `Active` lease, the lessor takes the entire collateral regardless
  of how much rent had actually accrued by then. This is a deliberate
  simplification — the `last_rent_paid_ts` bookkeeping in Fix 5 is in
  place precisely so a future version can split the pot correctly.

- **No pause / upgrade authority.** The program has no admin and no
  upgrade authority-bound feature flags. It runs or it doesn't.

---

## 7. Running the tests

All the tests are LiteSVM-based Rust integration tests under
[`programs/asset-leasing/tests/`](programs/asset-leasing/tests/). They
exercise every instruction through `include_bytes!("../../../target/deploy/asset_leasing.so")`,
so a fresh build must produce the `.so` first.

### Prerequisites

- Anchor 1.0.0 (`anchor --version`)
- Solana CLI (`solana -V`)
- Rust stable (the `rust-toolchain.toml` at the repo root pins the
  compiler)

### Commands

From this directory (`defi/asset-leasing/anchor/`):

```bash
# 1. Build the BPF .so — writes to target/deploy/asset_leasing.so
anchor build

# 2. Run the LiteSVM tests (just cargo under the hood; `anchor test`
#    also works because Anchor.toml scripts.test = "cargo test")
cargo test --manifest-path programs/asset-leasing/Cargo.toml

# Or, equivalently:
anchor test --skip-local-validator
```

Expected output:

```
running 11 tests
test close_expired_cancels_listed_lease ... ok
test close_expired_reclaims_collateral_after_end_ts ... ok
test create_lease_locks_tokens_and_lists ... ok
test create_lease_rejects_same_mint_for_leased_and_collateral ... ok
test liquidate_rejects_healthy_position ... ok
test liquidate_rejects_mismatched_price_feed ... ok
test liquidate_seizes_collateral_on_price_drop ... ok
test pay_rent_streams_collateral_by_elapsed_time ... ok
test return_lease_refunds_unused_collateral ... ok
test take_lease_posts_collateral_and_delivers_tokens ... ok
test top_up_collateral_increases_vault_balance ... ok
```

### What each test exercises

| Test | Exercises |
|---|---|
| `create_lease_locks_tokens_and_lists` | Lessor funds vault, `Lease` created, collateral vault empty |
| `create_lease_rejects_same_mint_for_leased_and_collateral` | Guard against `leased_mint == collateral_mint` |
| `take_lease_posts_collateral_and_delivers_tokens` | Collateral deposit + leased-token payout in one ix |
| `pay_rent_streams_collateral_by_elapsed_time` | Rent math: `elapsed * rent_per_second`, rent transferred to lessor |
| `top_up_collateral_increases_vault_balance` | Collateral balance after `top_up` equals deposit + top-up |
| `return_lease_refunds_unused_collateral` | Happy path round-trip — leased tokens returned, residual collateral refunded, accounts closed |
| `liquidate_seizes_collateral_on_price_drop` | Price-induced underwater position → rent + bounty + lessor share paid, accounts closed |
| `liquidate_rejects_healthy_position` | Program refuses to liquidate a position that passes the margin check |
| `liquidate_rejects_mismatched_price_feed` | Program refuses a `PriceUpdateV2` whose `feed_id` ≠ `lease.feed_id` |
| `close_expired_reclaims_collateral_after_end_ts` | Default path — lessor seizes the collateral |
| `close_expired_cancels_listed_lease` | Lessor-initiated cancel of an unrented lease |

### Note on CI

The repo's `.github/workflows/anchor.yml` runs `anchor build` before
`anchor test` for every changed anchor project. That's important for
this project: the Rust integration tests include the BPF artefact via
`include_bytes!`, so a stale or missing `.so` would break the tests.
CI is already covered.

---

## 8. Extending the program

A few directions that are genuinely educational rather than cargo-cult
extensions:

### Easy

- **Add a `lease_view` read-only helper.** An off-chain indexer-style
  struct that returns `{ collateral_value, debt_value, ratio_bps,
  is_underwater }` given the same inputs `is_underwater` uses. Useful
  for UIs that want to show "you are 15% away from liquidation".

- **Cap rent at collateral.** Currently `pay_rent` pays `min(rent_due,
  collateral_amount)` and silently leaves a debt. Add an explicit
  `RentDebtOutstanding` error so the caller is warned when the stream
  has stalled, rather than inferring it from a non-zero `rent_due`
  after settlement.

### Moderate

- **Partial-refund default.** In `close_expired` on `Active`, instead
  of giving the lessor the entire collateral, split it:
  `rent_due` to the lessor, the rest stays with the lessee up to some
  `default_haircut_bps`. `last_rent_paid_ts` is already bumped by
  Fix 5, so the timestamp invariants are ready.

- **Multiple outstanding leases per `(lessor, lessee)` pair with the
  same mint pair.** Already supported via `lease_id`, but add an
  instruction-level index account that lists open lease ids for a
  given lessor so off-chain tools don't have to `getProgramAccounts`
  scan.

- **Quote asset ≠ collateral mint.** Rent and liquidation math assume
  debt is priced in *collateral units*. Generalise to a third "quote"
  mint by taking the price pair at creation and carrying a
  `quote_mint` pubkey on `Lease`. Requires updates to
  `is_underwater` and a second oracle feed.

### Harder

- **Keeper auction.** Replace the fixed `liquidation_bounty_bps` with a
  Dutch auction that grows the bounty linearly over some window after
  the position first becomes underwater. Keeps liquidators honest on
  tight feeds and gives lessees a chance to `top_up_collateral` before
  a keeper has an economic reason to move.

- **Flash liquidation.** Let the keeper settle the debt in the same
  transaction as the liquidation — borrow the leased amount from a
  separate liquidity pool, hand it to the lessor, take the full
  collateral, repay the pool, keep the spread. Requires integrating a
  second program.

- **Token-2022 support.** The program already uses the `TokenInterface`
  trait so it accepts both SPL Token and Token-2022 mints. A real
  extension would test against Token-2022 mint extensions
  (transfer-fee, interest-bearing) and document which are compatible
  with the rent / collateral flows.

---

## Code layout

```
defi/asset-leasing/anchor/
├── Anchor.toml
├── Cargo.toml
├── README.md              (this file)
└── programs/asset-leasing/
    ├── Cargo.toml
    ├── src/
    │   ├── constants.rs    PDA seeds, bps limits, Pyth age cap
    │   ├── errors.rs
    │   ├── lib.rs          #[program] entry points
    │   ├── instructions/
    │   │   ├── mod.rs
    │   │   ├── shared.rs           transfer / close helpers
    │   │   ├── create_lease.rs
    │   │   ├── take_lease.rs
    │   │   ├── pay_rent.rs
    │   │   ├── top_up_collateral.rs
    │   │   ├── return_lease.rs
    │   │   ├── liquidate.rs
    │   │   └── close_expired.rs
    │   └── state/
    │       ├── mod.rs
    │       └── lease.rs
    └── tests/
        └── test_asset_leasing.rs   LiteSVM tests
```
