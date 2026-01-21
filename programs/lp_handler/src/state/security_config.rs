use anchor_lang::prelude::*;

/// 通用安全配置 PDA seeds（单例）
pub const SECURITY_CONFIG_SEED: &[u8] = b"security_config";

/// Pool 白名单最大容量（限制长度，避免账户过大；同时避免固定数组导致 SBF 栈溢出）
pub const MAX_ALLOWED_POOLS: usize = 64;

#[account]
pub struct SecurityConfig {
    /// 管理员
    pub authority: Pubkey,
    /// 允许的 pool_state 列表（长度受 MAX_ALLOWED_POOLS 限制）
    pub pools: Vec<Pubkey>,
}

impl SecurityConfig {
    pub const SIZE: usize = 8 + 32 + 4 + 32 * MAX_ALLOWED_POOLS;

    pub fn contains(&self, key: &Pubkey) -> bool {
        self.pools.iter().any(|p| p == key)
    }
}
