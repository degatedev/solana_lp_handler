use anchor_lang::prelude::*;

pub mod instructions;
pub mod state;

use instructions::*;
use state::*;

// ProgramId 需要与部署的 program keypair 对应的地址一致。
// 本分支统一使用生产环境（mainnet）的 ProgramId。
declare_id!("3e1bXXZBUSHuJK7i2szghnKkSxnQSSwbEgN8QfVPFxxj");

#[program]
pub mod lp_handler {
    use super::*;

    /// 先调用 Raydium swap_v2 换币，再调用 open_position_v2 开仓添加流动性
    /// 使用 Raydium 的 liquidity_math 精确计算最优 swap 比例
    #[allow(clippy::too_many_arguments)]
    pub fn swap_and_deposit<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, SwapAndDeposit<'info>>,
        deposit_amount: u64,
        deposit_mint: Pubkey,
        tick_lower_index: i32,
        tick_upper_index: i32,
        liquidity: i128,
        slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
    ) -> Result<()> {
        instructions::swap_and_deposit(
            ctx,
            deposit_amount,
            deposit_mint,
            tick_lower_index,
            tick_upper_index,
            liquidity,
            slippage_bps,
        )
    }

    pub fn decrease_liquidity<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
        liquidity: u128,
        mint_amount_0: u64,
        mint_amount_1: u64,
        swap_to_token_mint: Pubkey,
        slippage_bps: u16,
        fee_percent: u16,
        convert_to_usdc: bool,
    ) -> Result<()> {
        instructions::decrease_liquidity(
            ctx,
            liquidity,
            mint_amount_0,
            mint_amount_1,
            swap_to_token_mint,
            slippage_bps,
            fee_percent,
            convert_to_usdc,
        )
    }
}
