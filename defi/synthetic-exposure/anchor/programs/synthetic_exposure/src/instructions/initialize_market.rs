use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::{
    ASSET_SYMBOL_LEN, MARKET_SEED, MAX_ALLOWED_LEVERAGE_BPS, MAX_MAINTENANCE_MARGIN_BPS,
    VAULT_SEED,
};
use crate::errors::ErrorCode;
use crate::state::Market;

pub fn initialize_market(
    context: Context<InitializeMarketAccountConstraints>,
    asset_symbol: [u8; ASSET_SYMBOL_LEN],
    pyth_feed_id: [u8; 32],
    maintenance_margin_bps: u16,
    max_leverage_bps: u32,
) -> Result<()> {
    // Symbol must have at least one non-zero byte; pure zeros would mean a
    // client forgot to set it, and two such markets would share a PDA.
    require!(
        asset_symbol.iter().any(|byte| *byte != 0),
        ErrorCode::InvalidAssetSymbol
    );
    require!(
        maintenance_margin_bps > 0 && maintenance_margin_bps <= MAX_MAINTENANCE_MARGIN_BPS,
        ErrorCode::InvalidMarketConfig
    );
    require!(
        max_leverage_bps > 0 && max_leverage_bps <= MAX_ALLOWED_LEVERAGE_BPS,
        ErrorCode::InvalidMarketConfig
    );

    let market = &mut context.accounts.market;
    market.authority = context.accounts.authority.key();
    market.quote_mint = context.accounts.quote_mint.key();
    market.vault = context.accounts.vault.key();
    market.vault_bump = context.bumps.vault;
    market.pyth_feed_id = pyth_feed_id;
    market.maintenance_margin_bps = maintenance_margin_bps;
    market.max_leverage_bps = max_leverage_bps;
    market.total_long_size = 0;
    market.total_short_size = 0;
    market.asset_symbol = asset_symbol;
    market.bump = context.bumps.market;
    market.is_active = true;

    Ok(())
}

#[derive(Accounts)]
#[instruction(asset_symbol: [u8; ASSET_SYMBOL_LEN])]
pub struct InitializeMarketAccountConstraints<'info> {
    #[account(
        init,
        payer = authority,
        space = Market::DISCRIMINATOR.len() + Market::INIT_SPACE,
        // The asset symbol is fixed-length (padded to `ASSET_SYMBOL_LEN`) so
        // the seed is deterministic across clients.
        seeds = [MARKET_SEED, asset_symbol.as_ref()],
        bump
    )]
    pub market: Box<Account<'info, Market>>,

    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        payer = authority,
        // Vault is a PDA so clients can always derive it from the market
        // and don't need a separate signer keypair at init time. The market
        // PDA itself holds token authority so only CPIs from this program
        // can move funds out.
        seeds = [VAULT_SEED, market.key().as_ref()],
        bump,
        token::mint = quote_mint,
        token::authority = market,
        token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,

    pub system_program: Program<'info, System>,
}
