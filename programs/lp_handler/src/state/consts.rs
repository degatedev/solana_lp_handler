use anchor_lang::prelude::*;

/// 固定的 integrator fee 收款地址白名单（按编译 feature 区分环境）
///
/// - 默认（不带 feature）：devnet
/// - 编译时带 `--features mainnet`：mainnet
pub const FEE_OWNERS: &[Pubkey] = &[
    pubkey!("J3t7ucPyFEh4QH7ut1nfLe1JKC18NU476tyhxNsQvnqg"), // prod
    pubkey!("E32ykUTbi4Ag8t4Hic41HtDVwAZca1oGorqvkt3YS7Dy"), // dev
    pubkey!("5sMFtms83riv1vq7e2nGtDBN9w14Mvj2uojPNe32fvwE"), // teste
];

pub fn is_fee_owner(owner: &Pubkey) -> bool {
    FEE_OWNERS.iter().any(|k| k == owner)
}
