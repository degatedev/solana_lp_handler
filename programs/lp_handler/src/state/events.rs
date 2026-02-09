use anchor_lang::prelude::*;

/// Swap 执行事件

/// 流动性添加事件
#[event]
pub struct LpHandlerIncreaseLiquidityEvent {
    /// Pool 地址
    pub pool: Pubkey,
    /// Position NFT mint 地址
    pub position_nft_mint: Pubkey,
    /// 实际添加的 token0 数量
    pub principal_0: u64,
    /// 实际添加的 token1 数量
    pub principal_1: u64,
    /// token0 mint
    pub token0_mint: Pubkey,
    /// token1 mint
    pub token1_mint: Pubkey,
    /// 价格区间下限 tick
    pub tick_lower_index: i32,
    /// 价格区间上限 tick
    pub tick_upper_index: i32,
    /// 流动性值
    pub liquidity: u128,
    /// 用户输入的 token0 最大投入量（本次调用参数）
    pub amount_0_in: u64,
    /// 用户输入的 token1 最大投入量（本次调用参数）
    pub amount_1_in: u64,

    /// 最终退回给用户的 return_amount_0
    pub return_amount_0: u64,

    /// 最终退回给用户的 return_amount_1
    pub return_amount_1: u64,
}

#[event]
pub struct LpHandlerDecreaseLiquidityEvent {
    /// Pool 地址
    pub pool: Pubkey,

    /// token0 mint
    pub token0_mint: Pubkey,

    /// token1 mint
    pub token1_mint: Pubkey,

    /// 结算到的 mint：
    /// - None：不做转换，按 token0/token1 各自结算
    /// - Some(mint)：做转换（例如 USDC），所有 `*_settled_*` 只在目标币种一侧有值，另一侧为 0
    pub settle_mint: Option<Pubkey>,

    /// ==== 原始口径（pre）：不受 swap 影响，用于对账 ====
    /// 本金 token0 原始数量
    pub principal_pre_0: u64,
    /// 本金 token1 原始数量
    pub principal_pre_1: u64,
    /// 奖励 token0 原始数量（扣费前）
    pub reward_pre_fee_0: u64,
    /// 奖励 token1 原始数量（扣费前）
    pub reward_pre_fee_1: u64,

    /// ==== 结算口径（settled）：受转换影响，用于展示最终结果 ====
    /// 本金结算 token0 数量
    pub principal_settled_0: u64,
    /// 本金结算 token1 数量
    pub principal_settled_1: u64,
    /// 奖励结算 token0 数量
    pub reward_settled_0: u64,
    /// 奖励结算 token1 数量
    pub reward_settled_1: u64,
    /// 集成商收取的结算 token0 费用
    pub fee_settled_0: u64,
    /// 集成商收取的结算 token1 费用
    pub fee_settled_1: u64,
}

// ========== LPH-015: Security Config 配置变更事件 ==========

/// Security Config 初始化事件
#[event]
pub struct SecurityConfigInitialized {
    /// 管理员地址
    pub authority: Pubkey,
    /// 白名单 pool 列表
    pub pools: Vec<Pubkey>,
    /// 手续费接收者列表
    pub fee_owners: Vec<Pubkey>,
}

/// Security Config 更新事件
#[event]
pub struct SecurityConfigUpdated {
    /// 管理员地址
    pub authority: Pubkey,
    /// 更新后的 pool 列表
    pub pools: Vec<Pubkey>,
    /// 更新后的 fee_owners 列表
    pub fee_owners: Vec<Pubkey>,
}

/// Security Config 关闭事件
#[event]
pub struct SecurityConfigClosed {
    /// 管理员地址
    pub authority: Pubkey,
    /// 接收租金的地址
    pub receiver: Pubkey,
    /// 关闭前的 pool 数量
    pub pools_count: usize,
    /// 关闭前的 fee_owners 数量
    pub fee_owners_count: usize,
}
