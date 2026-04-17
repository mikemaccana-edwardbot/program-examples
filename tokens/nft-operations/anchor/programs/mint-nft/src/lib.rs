use anchor_lang::prelude::*;

declare_id!("3EMcczaGi9ivdLxvvFwRbGYeEUEHpGwabXegARw4jLxa");

pub mod instructions;

pub use instructions::*;

#[program]
pub mod mint_nft {

    use super::*;
    pub fn create_collection(mut context: Context<CreateCollection>) -> Result<()> {
        instructions::create_collection::handler(&mut context.accounts, &context.bumps)
    }

    pub fn mint_nft(mut context: Context<MintNFT>) -> Result<()> {
        instructions::mint_nft::handler(&mut context.accounts, &context.bumps)
    }

    pub fn verify_collection(mut context: Context<VerifyCollectionMint>) -> Result<()> {
        instructions::verify_collection::handler(&mut context.accounts, &context.bumps)
    }
}
