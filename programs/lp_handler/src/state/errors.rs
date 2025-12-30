use anchor_lang::prelude::*;

#[error_code]
pub enum LpDepositError {
    #[msg("Math operation overflow")]
    MathOverflow,

    #[msg("Invalid fee percent")]
    InvalidFeePercent,

    #[msg("Invalid slippage")]
    InvalidSlippage,

    #[msg("Invalid deposit mint")]
    InvalidDepositMint,

    #[msg("Invalid position nft owner")]
    InvalidPositionNftOwner,

    #[msg("Invalid position nft account")]
    InvalidPositionNftAccount,

    #[msg("Invalid fee token account")]
    InvalidFeeTokenAccount,

    #[msg("Invalid fee owner")]
    InvalidFeeOwner,

    #[msg("Invalid remaining accounts")]
    InvalidRemainingAccounts,

    #[msg("Invalid sqrt price")]
    InvalidSqrtPrice,

    #[msg("Invalid tick range")]
    InvalidTickRange,

    #[msg("No claimable rewards or redeemable principal")]
    NoBalanceChange,
}
