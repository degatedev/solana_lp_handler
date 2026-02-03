use anchor_lang::prelude::*;

// -----------------------------
// Security layer hardcoded config
//（不通过 PDA，直接写死在程序里）
// -----------------------------

/// 允许出现在账户列表中的“可执行程序账户（executable program account）”白名单。
///
/// 安全层会扫描本次指令可触达账户集合里所有 `executable == true` 的账户，
/// 若其 program id 不在本白名单中则直接拒绝。
pub const ALLOWED_EXECUTABLE_PROGRAMS: &[Pubkey] = &[
    crate::ID,
    anchor_lang::system_program::ID,
    anchor_spl::token::ID,
    anchor_spl::token_2022::ID,
    anchor_spl::associated_token::ID,
    anchor_spl::memo::ID,
    raydium_amm_v3::ID,
];

/// 允许作为“数据账户 owner(program id)”的白名单。
///
/// 用于检查：入口不存在、出口新初始化出来的账户，其 `owner` 必须在本集合内。
pub const ALLOWED_ACCOUNT_OWNERS: &[Pubkey] = &[
    anchor_lang::system_program::ID,
    anchor_spl::token::ID,
    anchor_spl::token_2022::ID,
    anchor_spl::associated_token::ID,
    raydium_amm_v3::ID,
];
