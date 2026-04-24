//! Test-only oracle writer.
//!
//! Creates (or overwrites) an account owned by this program and fills it
//! with bytes matching the `pyth_solana_receiver_sdk::PriceUpdateV2` layout,
//! so `synthetic_exposure` can read the account exactly as it would a real
//! Pyth price update.
//!
//! Originally this program existed as a companion to a live-validator test
//! suite: it owned the mock price accounts, and `synthetic_exposure` was
//! built with a `--features test-oracle` flag that relaxed its Pyth owner
//! check. That integration path has been retired.
//!
//! Under the current LiteSVM test suite the oracle accounts are seeded
//! directly with owner = Pyth Receiver via `LiteSVM::set_account`, so this
//! helper program is not needed at test runtime. The source remains as
//! executable documentation of the 134-byte `PriceUpdateV2` layout — the
//! same layout `synthetic_exposure::oracle` parses — and still builds as
//! part of the workspace.

use anchor_lang::prelude::*;

declare_id!("6Pxay9ELFPU1Wt2dimLE1ut95dZhQJM5CC73jZ9EXMk8");

/// Size of a Pyth `PriceUpdateV2` account. Matches
/// `pyth_solana_receiver_sdk::price_update::PriceUpdateV2::LEN` — copied
/// here rather than imported so `mock_pyth` has no production-SDK coupling.
const PRICE_UPDATE_V2_LEN: usize = 134;

/// Anchor discriminator for `PriceUpdateV2`. Precomputed as
/// `sha256("account:PriceUpdateV2")[..8]`. We hard-code it to avoid pulling
/// `sha2` as a dependency just for a constant value.
const PRICE_UPDATE_V2_DISCRIMINATOR: [u8; 8] = [34, 241, 35, 99, 157, 126, 244, 205];

#[program]
pub mod mock_pyth {
    use super::*;

    /// Write a fake Pyth `PriceUpdateV2` payload into the `price_update`
    /// account. The account must already exist and be owned by this
    /// program — our companion test harness creates it via the system
    /// program before calling `write_price`.
    pub fn write_price(
        context: Context<WritePrice>,
        price: i64,
        conf: u64,
        exponent: i32,
        publish_time: i64,
        feed_id: [u8; 32],
    ) -> Result<()> {
        let account = &context.accounts.price_update;
        let mut data = account.try_borrow_mut_data()?;
        // Account size is fixed at init; panicking here would be a caller
        // bug in the helper, so we return a clear error instead.
        require!(
            data.len() >= PRICE_UPDATE_V2_LEN,
            ErrorCode::AccountTooSmall
        );

        // Zero the account first so we overwrite any old state.
        for byte in data.iter_mut().take(PRICE_UPDATE_V2_LEN) {
            *byte = 0;
        }

        let mut offset = 0usize;

        // 8 bytes — Anchor discriminator.
        data[offset..offset + 8].copy_from_slice(&PRICE_UPDATE_V2_DISCRIMINATOR);
        offset += 8;

        // 32 bytes — write_authority. Not checked by the consumer; zero is fine.
        offset += 32;

        // 2 bytes — VerificationLevel. Variant 1 = `Full`, and the second
        // byte is padding (unused because `Full` carries no data, but
        // `PriceUpdateV2::LEN` reserves the wider `Partial` width).
        data[offset] = 1;
        data[offset + 1] = 0;
        offset += 2;

        // 32 bytes — feed_id.
        data[offset..offset + 32].copy_from_slice(&feed_id);
        offset += 32;

        // 8 bytes — price (i64, little-endian).
        data[offset..offset + 8].copy_from_slice(&price.to_le_bytes());
        offset += 8;

        // 8 bytes — conf (u64, little-endian).
        data[offset..offset + 8].copy_from_slice(&conf.to_le_bytes());
        offset += 8;

        // 4 bytes — exponent (i32, little-endian).
        data[offset..offset + 4].copy_from_slice(&exponent.to_le_bytes());
        offset += 4;

        // 8 bytes — publish_time (i64, little-endian).
        data[offset..offset + 8].copy_from_slice(&publish_time.to_le_bytes());
        offset += 8;

        // 8 bytes — prev_publish_time. Set equal to publish_time for simplicity.
        data[offset..offset + 8].copy_from_slice(&publish_time.to_le_bytes());
        offset += 8;

        // 8 bytes — ema_price. Mirror the live price.
        data[offset..offset + 8].copy_from_slice(&price.to_le_bytes());
        offset += 8;

        // 8 bytes — ema_conf. Mirror the live conf.
        data[offset..offset + 8].copy_from_slice(&conf.to_le_bytes());
        offset += 8;

        // 8 bytes — posted_slot. Not checked by our consumer; zero is fine.
        offset += 8;

        // Sanity check — offset must equal the known fixed length, otherwise
        // the layout changed upstream and this helper must be updated.
        require!(offset == PRICE_UPDATE_V2_LEN, ErrorCode::LayoutMismatch);

        Ok(())
    }

    /// Create the mock price account (system-program funded PDA) sized to
    /// hold a full `PriceUpdateV2` payload. Invoked once per feed id in
    /// tests; subsequent `write_price` calls reuse the same account.
    pub fn init_price(context: Context<InitPrice>, _feed_id: [u8; 32]) -> Result<()> {
        // Anchor's `init` constraint has already allocated and zeroed the
        // account. Nothing to do here.
        let _ = context;
        Ok(())
    }
}

#[error_code]
pub enum ErrorCode {
    #[msg("Target account is smaller than the Pyth PriceUpdateV2 layout")]
    AccountTooSmall,
    #[msg("Pyth PriceUpdateV2 layout offsets mismatched the declared length")]
    LayoutMismatch,
}

#[derive(Accounts)]
pub struct WritePrice<'info> {
    /// CHECK: owned by this program (enforced by runtime) and sized in
    /// `init_price`. We access its raw bytes directly to write the Pyth
    /// layout, so an Anchor account wrapper would be unhelpful here.
    #[account(mut, owner = crate::ID)]
    pub price_update: UncheckedAccount<'info>,

    pub payer: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(feed_id: [u8; 32])]
pub struct InitPrice<'info> {
    /// CHECK: initialized by the system program under this handler, then
    /// written by `write_price`. Seeds tie each account to a specific feed
    /// id so multiple feeds can coexist in one test suite.
    #[account(
        init,
        payer = payer,
        space = PRICE_UPDATE_V2_LEN,
        seeds = [b"mock_price", feed_id.as_ref()],
        bump,
    )]
    pub price_update: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}
