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
}
