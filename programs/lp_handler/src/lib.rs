#![allow(deprecated)]

use anchor_lang::prelude::*;

pub mod instructions;
pub mod security;
pub mod state;

use instructions::*;
use security as sec;
use state::*;

// ProgramId 需要与部署的 program keypair 对应的地址一致。
// 本分支统一使用生产环境（mainnet）的 ProgramId。
declare_id!("jQcuBroqSE5Gddwnsae5h7E3gnKwMMTGuLyGCASo6AW");

#[program]
#[allow(deprecated)]
pub mod lp_handler {
    use super::*;

    /// 强制所有入口使用同一套“入口检查 → 业务逻辑 → 出口检查”包装。
    ///
    /// - **user**: 本次指令的签名者（用于权限对账/黑名单）
    /// - **extra_authorities**: 业务允许出现的“新增 token account authority”白名单补充（如 `position_nft_owner`、`fee_owner`）
    /// - **pool_states**: 本次业务涉及的 pool_state（用于 pool 白名单校验）
    macro_rules! secure_entrypoint {
        (
            $ctx:expr,
            user = $user:expr,
            extra_authorities = $extra:expr,
            pool_states = $pools:expr,
            body = $body:expr
        ) => {{
            let remaining_accounts = $ctx.remaining_accounts.to_vec();
            let accounts = sec::collect_accounts_to_check(
                $ctx.accounts.to_account_infos(),
                &remaining_accounts,
            );
            let policy = sec::resolve_policy(&accounts)?;
            let snapshot = sec::entry_check_and_snapshot(
                &accounts,
                &remaining_accounts,
                $pools,
                $user,
                $extra,
                &policy,
            )?;
            let res = $body;
            sec::exit_check(&accounts, $user, $extra, &policy, snapshot)?;
            res
        }};
    }

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
        slippage_bps: u16,    // 滑点，单位为基点 (1 bps = 0.01%)
        lp_slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
    ) -> Result<()> {
        let user = ctx.accounts.user.key();
        let position_nft_owner = ctx.accounts.position_nft_owner.key();
        secure_entrypoint!(
            ctx,
            user = user,
            extra_authorities = &[position_nft_owner],
            pool_states = &[ctx.accounts.pool_state.key()],
            body = instructions::swap_and_deposit(
                ctx,
                deposit_amount,
                deposit_mint,
                tick_lower_index,
                tick_upper_index,
                liquidity,
                slippage_bps,
                lp_slippage_bps,
            )
        )
    }

    /// 减少 CLMM 流动性并按业务规则处理奖励/手续费。
    ///
    /// 注意：
    /// - 业务侧 `fee_owner` 的校验仍在指令实现里完成；
    /// - 安全层会做入口/出口对账（含 token authority/delegate/close_authority 等）。
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
        let user = ctx.accounts.user.key();
        let fee_owner = ctx.accounts.fee_owner.key();
        secure_entrypoint!(
            ctx,
            user = user,
            extra_authorities = &[fee_owner],
            pool_states = &[ctx.accounts.pool_state.key()],
            body = instructions::decrease_liquidity(
                ctx,
                liquidity,
                mint_amount_0,
                mint_amount_1,
                swap_to_token_mint,
                slippage_bps,
                fee_percent,
                convert_to_usdc,
            )
        )
    }
}
