use anchor_lang::prelude::*;

#[error_code(offset = 4000)]
pub enum LpDepositError {
    #[msg("Math operation overflow")]
    MathOverflow,

    #[msg("Invalid fee percent")]
    InvalidFeePercent,

    #[msg("Invalid slippage")]
    InvalidSlippage,

    #[msg("Invalid deposit mint")]
    InvalidDepositMint,

    #[msg("Invalid deposit amount")]
    InvalidDepositAmount,

    #[msg("Insufficient token balance")]
    InsufficientBalance,

    #[msg("Pool price out of range at execution: plan swap direction is incompatible with single-sided requirement")]
    OutOfRangeAtExecution,

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

    #[msg("Event log serialization failed")]
    EventLogSerializeFailed,

    #[msg("Invalid sqrt price")]
    InvalidSqrtPrice,

    #[msg("Invalid tick range")]
    InvalidTickRange,

    #[msg("No claimable rewards or redeemable principal")]
    NoBalanceChange,

    #[msg("Invalid security config: pools must not be empty")]
    SecurityConfigPoolsEmpty,

    // -----------------------------
    // Security layer errors
    // -----------------------------
    #[msg("Invalid remaining_accounts separator: lp_handler programId must appear exactly once and must be executable, non-signer, and non-writable")]
    SecuritySeparatorInvalid,

    #[msg(
        "Unauthorized executable program account found in transaction accounts (not in whitelist)"
    )]
    SecurityUnauthorizedExecutableProgram,

    #[msg("Pool not allowed: pool_state is not in ALLOWED_POOLS whitelist")]
    SecurityPoolNotAllowed,

    #[msg("Security config PDA missing: security_config PDA must be provided in accounts")]
    SecurityPoolWhitelistMissing,

    #[msg("Invalid security config PDA: owner is not this program or deserialization failed")]
    SecurityPoolWhitelistInvalid,

    #[msg("Security config admin unauthorized: authority is not admin")]
    SecurityConfigAdminUnauthorized,

    #[msg("Account set changed: an account present at entry is missing at exit (account set must be closed)")]
    SecurityAccountSetChanged,

    #[msg("Program-owned account has leaked SOL: lamports exceed rent-exempt minimum (owner==lp_handler)")]
    SecurityProgramLamportsLeaked,

    #[msg(
        "Token account corrupted: token account at entry cannot be parsed as token account at exit"
    )]
    SecurityTokenAccountCorrupted,

    #[msg("Token authority changed: user token account authority(owner) was modified (must remain user)")]
    SecurityTokenAuthorityChanged,

    #[msg("Token delegate not allowed (default policy forbids delegate)")]
    SecurityTokenDelegateNotAllowed,

    #[msg("Token close_authority not allowed (default policy forbids close_authority)")]
    SecurityTokenCloseAuthorityNotAllowed,

    #[msg("Disallowed new account owner: new account owner(program id) is not in ALLOWED_ACCOUNT_OWNERS whitelist")]
    SecurityDisallowedAccountOwner,

    #[msg("Invalid new token account authority: authority(owner) not in allowed set (user + extra_authorities; fee_owner privileged case excluded)")]
    SecurityNewTokenAccountAuthorityInvalid,

    #[msg(
        "Non-whitelisted token account: token account authority is not user or allowed authorities or pool supported token account"
    )]
    SecurityNonWhitelistTokenAccount,

    #[msg("Invalid recipient: recipient pubkey is not present in instruction accounts")]
    RecipientNotInAccounts,
}
