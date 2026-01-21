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

    // -----------------------------
    // Security layer errors
    // -----------------------------
    #[msg("remaining_accounts 分隔符无效：必须且只能出现一次 lp_handler programId，且该账户需 executable、非 signer、非 writable")]
    SecuritySeparatorInvalid,

    #[msg("交易账户列表包含未允许的可执行程序账户（executable program）。请检查是否引入了未白名单的第三方 program")]
    SecurityUnauthorizedExecutableProgram,

    #[msg("Pool 不在允许列表：pool_state 未在 ALLOWED_POOLS 白名单中")]
    SecurityPoolNotAllowed,

    #[msg("Token mint 不在允许列表：Token-2022 mint 未在 ALLOWED_TOKEN2022_MINTS 白名单中")]
    SecurityTokenMintNotAllowed,

    #[msg("Token-2022 mint 含高风险扩展（如 PermanentDelegate/TransferHook/Confidential/NonTransferable），已拒绝")]
    SecurityToken2022ForbiddenExtension,

    #[msg("缺少 Token-2022 mint 账户：无法读取扩展做风控（开启 Token-2022 白名单模式时必须把 mint account 传入）")]
    SecurityMissingToken2022MintAccount,

    #[msg("黑名单命中：检测到被拉黑的地址作为 user/authority/delegate/close_authority")]
    SecurityBlacklistedUser,

    #[msg("账户集合异常：入口快照中的账户在出口阶段缺失（账户集合应为闭包）")]
    SecurityAccountSetChanged,

    #[msg("程序自有账户滞留 SOL：owner==lp_handler 的账户 lamports 超过 rent-exempt 最小值")]
    SecurityProgramLamportsLeaked,

    #[msg(
        "Token 账户状态异常：入口为 token account，出口无法解析为 token account（可能被替换/损坏）"
    )]
    SecurityTokenAccountCorrupted,

    #[msg("Token 权限异常：用户 token account 的 authority(owner) 被更改（应保持为 user）")]
    SecurityTokenAuthorityChanged,

    #[msg("Token 权限异常：检测到 delegate（默认策略禁止设置 delegate）")]
    SecurityTokenDelegateNotAllowed,

    #[msg("Token 权限异常：检测到 close_authority（默认策略禁止设置 close_authority）")]
    SecurityTokenCloseAuthorityNotAllowed,

    #[msg("新初始化账户 owner 不合规：新账户的 owner(program id) 不在 ALLOWED_ACCOUNT_OWNERS 白名单中")]
    SecurityDisallowedAccountOwner,

    #[msg("新初始化 token account authority 不合规：其 authority(owner) 不在允许集合（user + extra_authorities；fee_owner 特权场景例外）")]
    SecurityNewTokenAccountAuthorityInvalid,
}
