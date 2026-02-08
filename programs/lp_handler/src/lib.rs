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
declare_id!("3jz7Mwbqk5RoQRbVSKdeNCLYwbmLgE3tARweHNZNTQQ9");

#[program]
#[allow(deprecated)]
pub mod lp_handler {
    use super::*;

    /// 强制所有入口使用同一套“入口检查 → 业务逻辑 → 出口检查”包装。
    ///
    /// - **signer**: 本次指令的签名者（安全层会对其 token accounts 做 authority/delegate/close_authority 对账）
    /// - **recipient**: 本次指令允许的“收款/接收方”（用于出口阶段允许新建 token account 的 authority）
    /// - **fee_owner**: 本次指令声明的手续费收款方（安全层会校验其必须在 security_config.fee_owners 白名单内）
    /// - **pool_state**: 本次业务涉及的 pool_state（用于 pool 白名单校验）
    macro_rules! secure_entrypoint {
        (
            $ctx:expr,
            signer = $signer:expr,
            recipient = $recipient:expr,
            fee_owner = $fee_owner:expr,
            pool_state = $pool:expr,
            body = $body:expr
        ) => {{
            // 内存优化取舍：
            // - 安全层主扫描对象仍以 ctx.accounts 为主；
            // - remaining_accounts 仍做入口阶段分隔符专项校验；
            // - 另外：会把 remaining_accounts 中 authority==signer 的 token accounts 纳入快照，
            //   并在出口对账时一并校验其权限变更（通过 AccountInfo clone 持有引用，避免在 body 之后再借用 ctx）。
            let accounts = $ctx.accounts.to_account_infos();
            let policy = sec::resolve_policy(&accounts)?;
            let snapshot = sec::entry_check_and_snapshot(
                &accounts,
                $ctx.remaining_accounts,
                $pool,
                $signer,
                $recipient,
                $fee_owner,
                &policy,
            )?;
            let res = $body;
            sec::exit_check(&accounts, $signer, $recipient, &policy, snapshot)?;
            res
        }};
    }

    /// 先调用 Raydium swap_v2 换币，再调用 open_position_v2 开仓添加流动性
    /// 使用 Raydium 的 liquidity_math 精确计算最优 swap 比例
    pub fn swap_and_deposit<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, SwapAndDeposit<'info>>,
        amount_0_in: u64,
        amount_1_in: u64,
        return_mint: Option<Pubkey>,
        tick_lower_index: i32,
        tick_upper_index: i32,
        slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
        swap_amount_in: u64,
        swap_min_out: u64,
        swap_input_is_token0: bool,
    ) -> Result<()> {
        let signer = ctx.accounts.signer.key();
        let recipient = ctx.accounts.recipient.key();
        let fee_owner = ctx.accounts.fee_owner.key();
        secure_entrypoint!(
            ctx,
            signer = signer,
            recipient = recipient,
            fee_owner = fee_owner,
            pool_state = &ctx.accounts.pool_state,
            body = instructions::swap_and_deposit(
                ctx,
                amount_0_in,
                amount_1_in,
                return_mint,
                tick_lower_index,
                tick_upper_index,
                slippage_bps,
                swap_amount_in,
                swap_min_out,
                swap_input_is_token0,
            )
        )
    }

    /// 减少 CLMM 流动性并按业务规则处理奖励/手续费。
    ///
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
        let signer = ctx.accounts.signer.key();
        let recipient = ctx.accounts.recipient.key();
        let fee_owner = ctx.accounts.fee_owner.key();
        secure_entrypoint!(
            ctx,
            signer = signer,
            recipient = recipient,
            fee_owner = fee_owner,
            pool_state = &ctx.accounts.pool_state,
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

    pub fn increase_liquidity<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, IncreaseLiquidity<'info>>,
        amount_0_in: u64,
        amount_1_in: u64,
        return_mint: Option<Pubkey>,
        tick_lower_index: i32,
        tick_upper_index: i32,
        slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
        swap_amount_in: u64,
        swap_min_out: u64,
        swap_input_is_token0: bool,
    ) -> Result<()> {
        let signer = ctx.accounts.signer.key();
        let fee_owner = ctx.accounts.fee_owner.key();
        secure_entrypoint!(
            ctx,
            signer = signer,
            recipient = signer,
            fee_owner = fee_owner,
            pool_state = &ctx.accounts.pool_state,
            body = instructions::increase_liquidity(
                ctx,
                amount_0_in,
                amount_1_in,
                return_mint,
                tick_lower_index,
                tick_upper_index,
                slippage_bps, // 滑点，单位为基点 (1 bps = 0.01%)
                swap_amount_in,
                swap_min_out,
                swap_input_is_token0,
            )
        )
    }

    /// 初始化 pool 白名单 PDA（单例）。
    /// 注意：该指令本身不走安全三明治包装（因为此时白名单可能尚未创建）。
    pub fn init_security_config(
        ctx: Context<InitSecurityConfig>,
        pools: Vec<Pubkey>,
        fee_owners: Vec<Pubkey>,
    ) -> Result<()> {
        instructions::init_security_config(ctx, pools, fee_owners)
    }

    /// 更新 pool 白名单 PDA（仅管理员可更新）。
    pub fn update_security_config(
        ctx: Context<UpdateSecurityConfig>,
        pools: Vec<Pubkey>,
        fee_owners: Vec<Pubkey>,
    ) -> Result<()> {
        instructions::update_security_config(ctx, pools, fee_owners)
    }

    /// 关闭 security_config PDA 并回收租金到 receiver。
    pub fn close_security_config(ctx: Context<CloseSecurityConfig>) -> Result<()> {
        instructions::close_security_config(ctx)
    }
}
