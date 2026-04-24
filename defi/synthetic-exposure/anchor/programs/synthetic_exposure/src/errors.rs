use anchor_lang::prelude::*;

/// Domain errors surfaced by the synthetic-exposure program.
///
/// Each variant carries a human-readable message that Anchor surfaces in
/// logs and simulation output, so clients can pattern-match on code or on
/// the message string as suits them.
#[error_code]
pub enum ErrorCode {
    #[msg("Swap must be in Created state for this action")]
    SwapNotCreated,

    #[msg("Swap must be in Active state for this action")]
    SwapNotActive,

    #[msg("Swap has already been settled or cancelled")]
    SwapAlreadyClosed,

    #[msg("Collateral offered by party B is below the required amount")]
    InsufficientCollateral,

    #[msg("Amount must be greater than zero")]
    ZeroAmount,

    #[msg("Required collateral is below the initial-margin floor")]
    CollateralBelowInitialMargin,

    #[msg("Taker fee exceeds the protocol maximum")]
    TakerFeeTooHigh,

    #[msg("Expiry timestamp must be strictly in the future")]
    ExpiryInPast,

    #[msg("Cannot settle before the swap's expiry timestamp")]
    NotYetExpired,

    #[msg("Position is still above maintenance margin — not liquidatable")]
    PositionHealthy,

    #[msg("Pyth price update is older than the allowed staleness window")]
    OracleStale,

    #[msg("Pyth confidence interval is wider than the protocol accepts")]
    OracleConfidenceTooWide,

    #[msg("Pyth price account's feed id does not match the swap's configured feed")]
    FeedIdMismatch,

    #[msg("Oracle reported a non-positive price; refusing to settle")]
    OracleNonPositive,

    #[msg("Pyth price account data is malformed or has an unexpected layout")]
    OracleMalformed,

    #[msg("Pyth price account is not owned by the expected program")]
    OracleWrongOwner,

    #[msg("Arithmetic overflow while computing swap math")]
    MathOverflow,

    #[msg("Caller is not authorised to perform this action")]
    Unauthorized,

    #[msg("Asset mint and quote mint must differ")]
    SameAssetAndQuote,
}
