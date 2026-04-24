use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("Requested leverage exceeds the market's configured maximum")]
    InvalidLeverage,

    #[msg("Vault balance is insufficient to pay out the requested amount")]
    InsufficientCollateral,

    #[msg("Position is still above its maintenance margin and cannot be liquidated")]
    PositionNotLiquidatable,

    #[msg("Pyth price update is older than the allowed staleness window")]
    OracleStale,

    #[msg("Pyth confidence interval is wider than the protocol accepts")]
    OracleConfidenceTooWide,

    #[msg("Pyth price account's feed id does not match the market's configured feed")]
    FeedIdMismatch,

    #[msg("Arithmetic overflow while computing PnL or collateral math")]
    MathOverflow,

    #[msg("Market is not active; no new trading permitted")]
    MarketInactive,

    #[msg("Caller is not authorised to perform this action")]
    Unauthorized,

    #[msg("Asset symbol must be non-empty and fit within the fixed seed length")]
    InvalidAssetSymbol,

    #[msg("Position size must be greater than zero")]
    ZeroSize,

    #[msg("Collateral must be greater than zero")]
    ZeroCollateral,

    #[msg("Market configuration out of range (leverage or margin beyond protocol limits)")]
    InvalidMarketConfig,

    #[msg("Oracle reported a non-positive price; refusing to trade")]
    OracleNonPositive,

    #[msg("Pyth price account data is malformed or has an unexpected layout")]
    OracleMalformed,

    #[msg("Pyth price account is not owned by the expected program")]
    OracleWrongOwner,
}
