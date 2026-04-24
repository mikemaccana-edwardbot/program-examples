//! Oracle reader — manually deserializes Pyth `PriceUpdateV2` bytes.
//!
//! The Pyth Solana Receiver SDK (`pyth-solana-receiver-sdk`) is not used as
//! a dependency because, at time of writing, it pins
//! `anchor-lang = "0.32.1"` which conflicts with the workspace's
//! `anchor-lang = "1.0.0"`. Rather than downgrade the whole program, we read
//! the 134-byte `PriceUpdateV2` layout directly — it is stable, documented,
//! and inexpensive to parse.
//!
//! The owner check enforces `PYTH_RECEIVER_PROGRAM_ID`
//! (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`) in all builds. There
//! used to be a `test-oracle` feature that relaxed this check for local
//! testing with a companion `mock_pyth` program; it was removed when the
//! integration tests moved to LiteSVM, which can seed an account with any
//! owner directly via `LiteSVM::set_account`. Production and test code
//! paths now read identical account metadata — no feature flags that
//! change program behaviour between the two.

use anchor_lang::prelude::*;

use crate::constants::{MAX_CONF_BPS, STALENESS_MAX_SECONDS};
use crate::errors::ErrorCode;

/// The real Pyth Solana Receiver program. Any price account read in
/// production must be owned by this program.
pub const PYTH_RECEIVER_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");

/// Anchor discriminator for `PriceUpdateV2`, computed once as
/// `sha256("account:PriceUpdateV2")[..8]`. Hard-coded so this module has no
/// `sha2` dependency.
pub const PRICE_UPDATE_V2_DISCRIMINATOR: [u8; 8] = [34, 241, 35, 99, 157, 126, 244, 205];

/// Minimum byte length required to cover every field up to and including
/// `exponent` and `publish_time`. Matches the on-chain layout:
///   8  discriminator
/// + 32 write_authority
/// + 2  verification_level (tag + max variant payload)
/// + 32 feed_id
/// + 8  price (i64)
/// + 8  conf (u64)
/// + 4  exponent (i32)
/// + 8  publish_time (i64)
/// = 102 bytes up through publish_time. The full account is 134 bytes.
pub const PRICE_UPDATE_V2_MIN_LEN: usize = 102;

/// A price snapshot pulled from the oracle account, matching the shape of
/// `pyth_solana_receiver_sdk::price_update::Price` without the dependency.
#[derive(Debug, Clone, Copy)]
pub struct OraclePrice {
    pub price: i64,
    pub conf: u64,
    pub exponent: i32,
    pub publish_time: i64,
}

/// Read and validate a price from the given account. Checks performed:
/// 1. Owner is the Pyth Receiver program.
/// 2. Minimum account size.
/// 3. Anchor discriminator for `PriceUpdateV2`.
/// 4. Feed id matches the market's configured feed.
/// 5. `publish_time` is within `STALENESS_MAX_SECONDS` of the current slot.
/// 6. Price is strictly positive.
/// 7. Confidence interval is within `MAX_CONF_BPS` of the price.
pub fn read_price(account: &UncheckedAccount, feed_id: &[u8; 32]) -> Result<OraclePrice> {
    verify_owner(account)?;

    let data = account.try_borrow_data()?;
    require!(
        data.len() >= PRICE_UPDATE_V2_MIN_LEN,
        ErrorCode::OracleMalformed
    );
    require!(
        data[..8] == PRICE_UPDATE_V2_DISCRIMINATOR,
        ErrorCode::OracleMalformed
    );

    // Byte-offsets mirror the `PriceUpdateV2` struct layout (see module
    // docstring). Each field is little-endian as produced by Borsh.
    let feed_id_bytes: [u8; 32] = data[42..74]
        .try_into()
        .map_err(|_| error!(ErrorCode::OracleMalformed))?;
    require!(feed_id_bytes == *feed_id, ErrorCode::FeedIdMismatch);

    let price = i64::from_le_bytes(
        data[74..82]
            .try_into()
            .map_err(|_| error!(ErrorCode::OracleMalformed))?,
    );
    let conf = u64::from_le_bytes(
        data[82..90]
            .try_into()
            .map_err(|_| error!(ErrorCode::OracleMalformed))?,
    );
    let exponent = i32::from_le_bytes(
        data[90..94]
            .try_into()
            .map_err(|_| error!(ErrorCode::OracleMalformed))?,
    );
    let publish_time = i64::from_le_bytes(
        data[94..102]
            .try_into()
            .map_err(|_| error!(ErrorCode::OracleMalformed))?,
    );

    // Staleness: match the Pyth SDK's rule — publish_time + max_age >= now.
    let now = Clock::get()?.unix_timestamp;
    let max_age_i64 = i64::try_from(STALENESS_MAX_SECONDS)
        .map_err(|_| error!(ErrorCode::MathOverflow))?;
    require!(
        publish_time.saturating_add(max_age_i64) >= now,
        ErrorCode::OracleStale
    );

    require!(price > 0, ErrorCode::OracleNonPositive);

    // Confidence check: `conf / |price| > MAX_CONF_BPS / 10_000` iff
    // `conf * 10_000 > |price| * MAX_CONF_BPS`. Pure integer math.
    let price_abs = (price.unsigned_abs()) as u128;
    let conf_u128 = conf as u128;
    let conf_scaled = conf_u128
        .checked_mul(10_000u128)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    let conf_limit = price_abs
        .checked_mul(MAX_CONF_BPS)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
    require!(
        conf_scaled <= conf_limit,
        ErrorCode::OracleConfidenceTooWide
    );

    Ok(OraclePrice {
        price,
        conf,
        exponent,
        publish_time,
    })
}

/// Owner check — enforced identically in prod and test builds. Tests seed
/// Pyth-owned accounts directly via `LiteSVM::set_account`, so no feature
/// flag is needed to satisfy it.
fn verify_owner(account: &UncheckedAccount) -> Result<()> {
    require_keys_eq!(
        *account.owner,
        PYTH_RECEIVER_PROGRAM_ID,
        ErrorCode::OracleWrongOwner
    );
    Ok(())
}
