use anchor_lang::prelude::*;

/// 固定的 integrator fee 收款地址白名单（生产环境）
pub const FEE_OWNERS: &[Pubkey] = &[
    pubkey!("J3t7ucPyFEh4QH7ut1nfLe1JKC18NU476tyhxNsQvnqg"), // prod
    pubkey!("E32ykUTbi4Ag8t4Hic41HtDVwAZca1oGorqvkt3YS7Dy"), // dev
    pubkey!("5sMFtms83riv1vq7e2nGtDBN9w14Mvj2uojPNe32fvwE"), // teste
    pubkey!("ECcfQBco4MLM8Dkztn7HSVyZze2MY3qkv621PYXbkvon"), // testd
    pubkey!("AyVjdDnjmkcsfLKZxwHVVafFDJ8BFhk5oLy3w9PLtfbY"), // testd
    pubkey!("5supVqBoki4jARg3EFjFgpP84mXs6PgC2nUwae3iTDcE"), // stg
];

pub fn is_fee_owner(owner: &Pubkey) -> bool {
    FEE_OWNERS.iter().any(|k| k == owner)
}

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

/// 允许的 Raydium pool_state 白名单（空 = 不限制）
pub const ALLOWED_POOLS: &[Pubkey] = &[
    // pubkey!("3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv"), // example
];

/// 黑名单用户（空 = 不限制）
pub const USER_BLACKLIST: &[Pubkey] = &[];

/// 允许的 Token-2022 mint（空 = 不限制；仍会做扩展风险检查）
pub const ALLOWED_TOKEN2022_MINTS: &[Pubkey] = &[];
