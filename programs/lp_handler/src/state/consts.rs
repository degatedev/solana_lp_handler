use anchor_lang::prelude::*;

// -----------------------------
// Security layer static allowlists
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

// SecurityConfig 初始化管理员。
// 构建时必须显式注入 `SECURITY_ADMIN` 环境变量；
// 发布脚本会在 `anchor build` 前注入当前部署钱包地址。
include!(concat!(env!("OUT_DIR"), "/security_admin.rs"));

pub const USDC_MINT: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

pub const MIN_USDC_SWAP_AMOUNT: u64 = 1000;

pub fn is_native_sol_mint(mint: &Pubkey) -> bool {
    *mint == anchor_spl::token::spl_token::native_mint::ID
        || *mint == anchor_spl::token_2022::spl_token_2022::native_mint::ID
}
