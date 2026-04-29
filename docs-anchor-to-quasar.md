# Converting an Anchor Program to Quasar

Quasar is a newer Solana program framework with Anchor-like surface syntax (`#[program]`, `#[derive(Accounts)]`, `#[account]`) but vastly better runtime performance: zero-copy account access, `no_std` by default, dense one-byte instruction discriminators, and compiled `.so` files roughly an order of magnitude smaller than Anchor's. You port for the compute-unit savings, the smaller binary, and explicit control over wire format. This document is a checklist; work through it in order. Examples are taken verbatim from the asset-leasing port (`/opt/clawsimple/data/program-examples/defi/asset-leasing/{anchor,quasar}/`) and the basics counter port (`/opt/clawsimple/data/program-examples/basics/counter/{anchor,quasar}/`).

## Project scaffolding

- Replace `Anchor.toml` with `Quasar.toml`. Build with `quasar build` instead of `anchor build`.
- The Quasar project is a flat Cargo package, not the Anchor `programs/<name>/` nested layout. `src/lib.rs` lives at the project root, not under `programs/`.
- Declare a standalone Cargo workspace inside the Quasar project (`[workspace]`) so it does not get pulled into the Anchor workspace's resolver.
- Tests run via `cargo test`, declared in `[testing.rust]`. No `anchor test`, no `npm test` for the program side.
- Generated client crates land in `target/client/rust/` per `[clients]`.

Anchor `Anchor.toml`:

```toml
[programs.localnet]
asset_leasing = "HHKEhLk6dyzG4mK1isPyZiHcEMW4J1CRKryzyQ3JFtnF"

[scripts]
test = "cargo test"
```

Quasar `Quasar.toml`:

```toml
[project]
name = "quasar_asset_leasing"

[toolchain]
type = "solana"

[testing]
language = "rust"

[testing.rust]
framework = "quasar-svm"

[testing.rust.test]
program = "cargo"
args = ["test", "tests::"]

[clients]
path = "target/client"
languages = ["rust"]
```

## Cargo.toml differences

- Drop `anchor-lang` and `anchor-spl`. Add `quasar-lang` and (for SPL token work) `quasar-spl`.
- No IDL feature flags. Quasar generates client crates from the build itself, not from an emitted IDL. The `idl-build` / `no-idl` / `no-log-ix-name` features all disappear.
- The crate-level `#![cfg_attr(not(test), no_std)]` attribute belongs at the top of `src/lib.rs`. The `not(test)` is the bit that lets your Rust tests still use `std`.
- Add a standalone `[workspace]` block so Cargo does not glue the Quasar crate into a parent workspace with conflicting resolver settings.
- Common feature flags on Quasar programs are `alloc`, `client`, and `debug`. `client` gates client-crate generation; the build uses it via `quasar build --features client`.

Anchor `Cargo.toml`:

```toml
[features]
default = []
idl-build = ["anchor-lang/idl-build", "anchor-spl/idl-build"]

[dependencies]
anchor-lang = { version = "1.0.0", features = ["init-if-needed"] }
anchor-spl = "1.0.0"
```

Quasar `Cargo.toml`:

```toml
[workspace]

[features]
alloc = []
client = []
debug = []

[dependencies]
quasar-lang = "0.0"
quasar-spl = "0.0"
solana-address = { version = "2.2.0" }
solana-instruction = { version = "3.2.0" }

[dev-dependencies]
quasar-svm = { version = "0.1" }
```

## `#[program]` module

- Every handler needs an explicit `#[instruction(discriminator = N)]` attribute. Number them densely from `0` and keep the order matching the user-facing lifecycle.
- Function signature: `Ctx<T>` not `Context<T>`, return type `Result<(), ProgramError>` not `Result<()>`. The `Ctx` carries `accounts`, `bumps`, and (for `init`) the bumps struct.
- Bodies forward to `instructions::handle_<name>(&mut ctx.accounts, ...)`. The accounts struct is mutable because handlers write into Pod-wrapped fields directly.
- The `mod` is private (`mod quasar_asset_leasing` not `pub mod`). Quasar's `#[program]` macro expands the entrypoints; nothing outside the crate needs to name it.

Anchor:

```rust
#[program]
pub mod asset_leasing {
    use super::*;

    pub fn take_lease(context: Context<TakeLease>) -> Result<()> {
        instructions::take_lease::handle_take_lease(context)
    }
}
```

Quasar:

```rust
#[program]
mod quasar_asset_leasing {
    use super::*;

    #[instruction(discriminator = 1)]
    pub fn take_lease(context: Ctx<TakeLease>) -> Result<(), ProgramError> {
        instructions::handle_take_lease(&mut context.accounts)
    }
}
```

## Accounts structs

- `<'info>` lifetime is mandatory on the struct.
- Fields are `&'info` references (or `&'info mut` for mutable accounts), not `Account<'info, T>` wrappers. Quasar's `Account<T>` is the inner type; the borrow lives on the field.
- `Signer`, `UncheckedAccount`, `Program<P>`, `Sysvar<S>` still exist but are bare types, used through `&'info` references.
- Each accounts struct gets an `impl<'info>`-style handler taking `&mut Self` and returning `Result<(), ProgramError>`. Mark it `#[inline(always)]` to keep the call cheap.
- `init` works the same way (`mut, init, payer = ..., seeds = [...], bump`) and the macro generates a `<StructName>Bumps` type passed in as `&context.bumps`.
- Quasar's `Account<Token>` (from `quasar-spl`) replaces `Box<InterfaceAccount<TokenAccount>>`. The `Box` is gone because there is no heap.

Anchor:

```rust
#[derive(Accounts)]
pub struct TakeLease<'info> {
    #[account(mut)]
    pub short_seller: Signer<'info>,

    #[account(
        mut,
        seeds = [LEASE_SEED, holder.key().as_ref(), &lease.lease_id.to_le_bytes()],
        bump = lease.bump,
        has_one = holder,
    )]
    pub lease: Account<'info, Lease>,

    pub leased_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_program: Interface<'info, TokenInterface>,
}
```

Quasar:

```rust
#[derive(Accounts)]
pub struct TakeLease<'info> {
    #[account(mut)]
    pub lessee: &'info Signer,

    pub lessor: &'info UncheckedAccount,

    #[account(
        mut,
        seeds = [LEASE_SEED, lessor],
        bump = lease.bump,
        has_one = lessor,
    )]
    pub lease: &'info mut Account<Lease>,

    pub leased_mint: &'info Account<Mint>,
    pub token_program: &'info Program<Token>,
}
```

## State structs

- `#[account]` becomes `#[account(discriminator = N)]`. You pick the byte; Quasar replaces Anchor's 8-byte SHA256 prefix with a single byte you control.
- Field order is load-bearing. Account data is pointer-cast directly from the SVM input buffer, so layout drift breaks deserialisation silently. Add new fields at the END.
- Numeric fields stay declared as `u64` / `i64` / `u32` / `u16` / `bool` in the source struct. The `#[account]` macro promotes them to their `PodU64` / `PodI64` / `PodU32` / `PodU16` / `PodBool` zero-copy counterparts under the hood. You declare the natural type but the in-memory representation is the Pod wrapper.
- `LeaseStatus` enums become a `u8` field on the account, with a separate `from_u8` helper. `#[derive(InitSpace)]` is gone (size is computed from the layout).
- For dynamic data, use `String<P, N>` / `Vec<T, P, N>` (length-prefix and max capacity) or the newer `PodString<N>` / `PodVec<T, N>` for fixed-capacity inline storage. No realloc, ever.

Anchor:

```rust
#[account]
#[derive(InitSpace)]
pub struct Lease {
    pub lease_id: u64,
    pub holder: Pubkey,
    pub leased_amount: u64,
    pub status: LeaseStatus,
    pub bump: u8,
}
```

Quasar:

```rust
#[account(discriminator = 1)]
pub struct Lease {
    pub lease_id: u64,
    pub lessor: Address,
    pub leased_amount: u64,
    pub status: u8,
    pub bump: u8,
}
```

## Reading and writing Pod fields

- Read with `.get()` (returns the unwrapped primitive) or with `.into()` and a type annotation. Both work; `.get()` is shorter when the call sits inline.
- Write by converting back: `field = value.into()` or `field = PodU64::from(value)`.
- `Address` (Quasar's pubkey) reads through `.address()` on accounts, returning `&Address`. Dereference with `*` to copy.
- This is where the bulk of porting bugs live. A read that compiles without the conversion produces the Pod wrapper, not the primitive, and downstream arithmetic gives wrong results.

Anchor:

```rust
ctx.accounts.counter.count = ctx.accounts.counter.count.checked_add(1).unwrap();
```

Quasar (counter port):

```rust
let current: u64 = accounts.counter.count.into();
accounts.counter.count = PodU64::from(current.checked_add(1).unwrap());
```

Quasar (asset-leasing port, mixed style):

```rust
let leased_amount = accounts.lease.leased_amount.get();
let collateral_amount = accounts.lease.collateral_amount.get();
lease.last_paid_timestamp = now.into();
```

## Sysvars

- `Clock::get()?` becomes `<Clock as quasar_lang::sysvars::Sysvar>::get()?`, and the returned timestamp is itself Pod-wrapped, so add `.get()` (or `.into()`) on the field.

Anchor:

```rust
let now = Clock::get()?.unix_timestamp;
```

Quasar:

```rust
let now = <Clock as quasar_lang::sysvars::Sysvar>::get()?.unix_timestamp.get();
```

## Constants and discriminators

- Plain `pub const FOO: &[u8] = b"...";` constants port with no changes; lifetime-elided `&'static [u8]` works the same.
- Account discriminators are explicit single bytes per `#[account(discriminator = N)]`. Pick them densely starting from 1 (0 is conventionally avoided so an uninitialised account never matches).
- Instruction discriminators are explicit single bytes per `#[instruction(discriminator = N)]`. Pick them densely starting from 0.
- The wire format is `[discriminator: u8][borsh-serialised args]`. No 8-byte prefix.

## Errors

- `#[error_code]` exists in Quasar (the asset-leasing port uses it directly) but it does not accept Anchor's `#[msg("...")]` attribute. Strip the message attributes; keep the variant names.
- Quasar's `#[error_code]` produces an enum that converts into `ProgramError` via `From`, so `Err(MyError::Foo.into())` and `?` chaining work the same as Anchor.
- Error codes start at 6000 to avoid collision with `ProgramError` and the framework's own `QuasarError`.

Anchor:

```rust
#[error_code]
pub enum AssetLeasingError {
    #[msg("Lease is not in the required state for this action")]
    InvalidLeaseStatus,
    #[msg("Arithmetic overflow")]
    MathOverflow,
}
```

Quasar:

```rust
#[error_code]
pub enum AssetLeasingError {
    InvalidLeaseStatus,
    MathOverflow,
}
```

## CPI conversions

- No `CpiContext::new()`. SPL token operations use a builder on the program handle: `self.token_program.transfer(from, to, authority, amount).invoke()`.
- The `quasar-spl` crate exports `Mint`, `Token`, `TokenCpi`. Bring the `TokenCpi` trait into scope to use the helpers.
- For program-derived address-signed CPIs, use `.invoke_signed(seeds)` instead of Anchor's `.with_signer(signer_seeds).invoke()`. Seeds are an `&[Seed]` slice built from `Seed::from(...)`.
- Generic CPIs to arbitrary programs use `BufCpiCall::new(...).invoke()`. Define a marker type for the foreign program: `pub struct MyProgram; impl Id for MyProgram { const ID: Address = ... }`.
- `close_account` is also a `TokenCpi` method: `token_program.close_account(account, destination, authority).invoke_signed(seeds)`.

Anchor:

```rust
let signer_seeds: &[&[&[u8]]] = &[&[
    LEASED_VAULT_SEED,
    lease_key.as_ref(),
    &[leased_vault_bump],
]];
let cpi = CpiContext::new_with_signer(
    token_program.to_account_info(),
    TransferChecked { ... },
    signer_seeds,
);
transfer_checked(cpi, amount, decimals)?;
```

Quasar:

```rust
let leased_vault_bump = [accounts.lease.leased_vault_bump];
let lease_address = *accounts.lease.address();
let vault_seeds: &[Seed] = &[
    Seed::from(LEASED_VAULT_SEED),
    Seed::from(lease_address.as_ref()),
    Seed::from(&leased_vault_bump as &[u8]),
];
accounts
    .token_program
    .transfer(
        accounts.leased_vault,
        accounts.lessee_leased_account,
        accounts.leased_vault,
        leased_amount,
    )
    .invoke_signed(vault_seeds)?;
```

## Logging

- `log("static string only")`. No format strings, no `msg!()`, no interpolation, ever.
- If you need to log a value, issue multiple `log()` calls with separate static strings, or log raw bytes.
- This applies to error messages too: `#[error_code]` variants do not carry a human-readable message in Quasar.

## Constraint vocabulary

What survives:

- `mut`
- `init` (with `payer`, `seeds`, `bump`)
- `seeds`, `bump`, `bump = lease.bump`
- `payer = ...`
- `has_one = field`
- `constraint = expr @ ErrorVariant`
- `close = recipient` (yes, despite older notes; the asset-leasing port uses it on `liquidate`, `return_lease`, and `close_expired`)
- `token::mint = ...`, `token::authority = ...` on `init` for new vaults

What does NOT survive:

- `init_if_needed` (no Quasar equivalent; pre-create token accounts off-chain or do the CPI yourself)
- `realloc`, `realloc::payer`, `realloc::zero`
- `associated_token::mint`, `associated_token::authority`, `associated_token::token_program` (no automatic associated token account creation; the caller passes pre-created accounts)
- `token::*` enforcement constraints on existing (non-`init`) accounts (verify mints and authorities manually inside the handler if you need them)
- The `Box<...>` wrapper on accounts (no heap; not needed)

## No `realloc` constraint

- Plan the layout up front. Pick a maximum capacity for any dynamic field and use `PodString<N>` or `PodVec<T, N>` for fixed-capacity inline storage.
- `String<P, N>` and `Vec<T, P, N>` (length-prefixed dynamic) exist but write through a slower realloc path internally; new code should prefer the Pod variants.
- New fields go at the END of the struct. Layout is part of your account's wire format.

## Manual mint and authority checks

- Anchor's `token::mint = leased_mint` constraint disappears. If you need to enforce that a passed-in token account holds a specific mint, read the field inside the handler and compare.
- Same for `token::authority = ...`. The borrow checker on Quasar's pointer-cast accounts makes these awkward inline, so push them into a small helper if a single handler needs to verify both vaults.

## Tests

- Quasar uses `quasar-svm` for in-process Rust tests. Pure Rust, no JavaScript, no LiteSVM, no Bankrun.
- Tests live in `src/tests.rs` and run via `cargo test`. The harness is set up by `[testing.rust]` in `Quasar.toml` but the actual runner is just `cargo test tests::`.
- Tests load the compiled `.so` via `include_bytes!("../target/deploy/quasar_<name>.so")`, so a fresh `quasar build` must run before `cargo test`.
- Tests build instructions either by hand (raw `solana_instruction::AccountMeta` + a manually-assembled byte payload) or via the auto-generated client crate (`<Name>Instruction { ... }.into()`).
- The Anchor side of program-examples now uses LiteSVM-style Rust tests too. The framework split is worth flagging: the Anchor LiteSVM tests use `solana-kite` plus `borsh` decoding; the Quasar tests use `quasar-svm` helpers like `quasar_svm::token::create_keyed_system_account` and direct byte-level account assertions.

Quasar test setup:

```rust
fn setup() -> QuasarSvm {
    let elf = include_bytes!("../target/deploy/quasar_counter.so");
    QuasarSvm::new().with_program(&Pubkey::from(crate::ID), elf)
}
```

## Client generation

- `quasar build --features client` produces a Rust client crate under `target/client/rust/<name>-client`. No Codama step.
- Each instruction handler gets a generated struct with named fields (one per account in the `#[derive(Accounts)]` struct, plus the instruction args). The struct implements `Into<solana_instruction::Instruction>`.
- For TypeScript clients, you still need a separate codegen step. Quasar does not emit an Anchor-compatible IDL out of the box; if your offchain stack is TypeScript, plan for a manual or hand-rolled client until the IDL story matures.
- The auto-generated Rust client makes the Rust test harness much less verbose than driving raw `AccountMeta` slices.

## Same program ID

- Keep the same `declare_id!()` value across the Anchor and Quasar binaries. Asset-leasing puts the same `Lease11111...` ID on both sides.
- Program-derived addresses derive from the program ID, so matching IDs means PDA derivations, RPC lookups, and any offchain indexer keyed on program ID work against either build with no client changes.
- During development you will deploy only one binary at a time to localnet. The two `.so` files are interchangeable as far as the chain is concerned (modulo discriminator differences).

## What does NOT need to change

- Seeds: byte-string constants and the conceptual seed scheme port unchanged.
- Bumps: stored as `u8` on state accounts the same way.
- Borsh-serialised primitive instruction args (`u64`, `[u8; 32]`, etc) have the same wire format. Argument lists on handlers stay byte-compatible.
- Most program logic. The flow of `create_lease`, `take_lease`, `pay_lease_fee`, etc, is identical line-for-line; the changes are mechanical (Pod conversions, CPI builder syntax, lifetime annotations, `Ctx` vs `Context`).
- Error variant names. Strip `#[msg("...")]`, keep the names so callers and tests stay readable.

## Common porting pitfalls

- Forgetting `.get()` / `.into()` on a Pod read. Compiles, runs, gives wrong values.
- Adding a new field anywhere except the end of a state struct. Silent layout breakage.
- Trying to format inside `log()`. Will not compile under `no_std`.
- Missing `<'info>` on the accounts struct or on a field. Confusing borrow-checker error far from the cause.
- Reaching for `init_if_needed` on a token account. Does not exist; either pre-create the account off-chain or wire the associated token account program CPI yourself.
- Trying to use `realloc`. Pick the max capacity up front and use `PodString<N>` / `PodVec<T, N>`.
- Using `token::mint = ...` on a non-`init` account. The constraint is silently ignored on existing accounts; verify inside the handler instead.
- Borrowing `accounts.lease` mutably while still needing a field from it. Bind the values you need to locals first (`let leased_vault_bump = accounts.lease.leased_vault_bump;`), then take the `&mut`.
- Embedding instruction args into `seeds = [...]` (e.g. `&lease_id.to_le_bytes()`). The macro currently does not have a borrow-safe way to splice instruction args into the seed list, so program-derived address schemes that key on a runtime-supplied byte slice need to be reworked or simplified. The asset-leasing port dropped multi-lease-per-lessor support for exactly this reason.
- Forgetting to bring `quasar_spl::TokenCpi` into scope. The `.transfer(...)` builder method is on the trait, not the type.
- Forgetting `&context.bumps` on `init` handlers. The bumps struct is generated by `#[derive(Accounts)]` and has to be threaded through to the handler explicitly.
