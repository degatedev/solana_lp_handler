use anchor_lang::prelude::*;

/// Swap 执行事件
#[event]
pub struct SwapExecutedEvent {
    /// 执行 swap 的用户
    pub user: Pubkey,
    /// Pool 地址
    pub pool: Pubkey,
    /// Swap 输入数量
    pub amount_in: u64,
    /// Swap 输出数量
    pub amount_out: u64,
    /// Swap 最小输出数量
    pub amount_out_min: u64,
    /// token0 mint
    pub token0_mint: Pubkey,
    /// token1 mint
    pub token1_mint: Pubkey,
    /// 是否为 token0 输入
    pub is_token0_input: bool,
    /// 滑点（基点）
    pub slippage_bps: u16,
}

/// 流动性添加事件
#[event]
pub struct IncreaseLiquidityEvent {
    /// 添加流动性的用户
    pub user: Pubkey,
    /// Pool 地址
    pub pool: Pubkey,
    /// Position NFT mint 地址
    pub position_nft_mint: Option<Pubkey>,
    /// 实际添加的 token0 数量
    pub amount_0: u64,
    /// 实际添加的 token1 数量
    pub amount_1: u64,
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
    /// 预期投入的数量
    pub deposit_amount: u64,
    /// 返回的数量
    pub return_amount: u64,
}

#[event]
pub struct DecreaseLiquidityEvent {
    /// 减少流动性的用户
    pub user: Pubkey,
    /// Pool 地址
    pub pool: Pubkey,

    /// token0 mint
    pub token0_mint: Pubkey,

    /// token1 mint
    pub token1_mint: Pubkey,

    /// 实际减少的 token0 数量
    pub principal_amount_0: u64,
    /// 实际减少的 token1 数量
    pub principal_amount_1: u64,

    /// 集成商收取的 token0 费用
    pub integrator_fee_0: u64,

    /// 集成商收取的 token1 费用
    pub integrator_fee_1: u64,

    pub reward_amount_0: u64,

    /// 奖励 token0 数量
    pub reward_amount_1: u64,
}
