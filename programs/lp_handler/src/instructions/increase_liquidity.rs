use anchor_lang::prelude::*;
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{
    AmmConfig, ObservationState, PersonalPositionState, PoolState, TickArrayState,
};

use crate::SECURITY_CONFIG_SEED;

use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

use super::zap_common;

/// increase_liquidity 所需的所有账户
/// 包含 `swap_v2` 与 `increase_liquidity_v2` 的全部账户（有些可以复用，比如 pool_state、token_program 等）
#[derive(Accounts)]
#[instruction(
    amount_0_in: u64,
    amount_1_in: u64,
    return_mint: Option<Pubkey>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
    swap_amount_in: u64,
    swap_min_out: u64,
    swap_input_is_token0: bool,
)]
pub struct IncreaseLiquidity<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program (主网: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK)
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// 支付者 / 签名者
    #[account(mut)]
    pub signer: Signer<'info>,

    #[account(
        mut,
        token::mint = token_vault_0.mint,
        token::authority = signer,
    )]
    pub signer_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = signer,
    )]
    pub signer_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        constraint = position_nft_account.mint == personal_position.nft_mint,
        constraint = position_nft_account.amount == 1,
        token::authority = signer
    )]
    pub position_nft_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 为该 position 增加流动性
    #[account(mut, constraint = personal_position.pool_id == pool_state.key())]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// CHECK: `protocol_position` 已废弃，仅为兼容保留
    pub protocol_position: UncheckedAccount<'info>,

    /// lower tick 对应的 TickArray 状态（由 Raydium 使用）
    #[account(mut, constraint = tick_array_lower.load()?.pool_id == pool_state.key())]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// upper tick 对应的 TickArray 状态（由 Raydium 使用）
    #[account(mut, constraint = tick_array_upper.load()?.pool_id == pool_state.key())]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool 状态账户（swap 需要）
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Observation 状态（swap 需要）
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    #[account(mut)]
    pub fee_owner: SystemAccount<'info>,

    #[account(mut,
        token::mint = token_vault_0.mint,
        token::authority = fee_owner,
    )]
    pub fee_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = fee_owner,
    )]
    pub fee_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub memo_program: Program<'info, Memo>,

    /// 池子 token_0 的金库 TokenAccount 地址
    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 池子 token_1 的金库 TokenAccount 地址
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Rent sysvar（用于 mint/ATA 创建等租金相关逻辑）
    pub rent: Sysvar<'info, Rent>,

    /// 系统程序（创建/分配账户）
    pub system_program: Program<'info, System>,

    /// SPL Token 程序（Token-2022 之外的转账等）
    pub token_program: Program<'info, Token>,

    /// ATA 程序（用于创建 position NFT 的接收 ATA）
    pub associated_token_program: Program<'info, AssociatedToken>,

    /// Token-2022 程序（用于 position NFT 的 mint/token account/转账等）
    pub token_program_2022: Program<'info, Token2022>,

    /// CHECK: 通用安全配置 PDA（必须传入），允许“未初始化”的 system-owned 空账户。
    /// - 地址通过 seeds+bump 校验为固定 PDA
    /// - 是否初始化由安全层在运行时判断；未初始化时跳过 pool 白名单校验
    #[account(
        seeds = [SECURITY_CONFIG_SEED],
        bump
    )]
    pub security_config: UncheckedAccount<'info>,

    /// token vault 0 的 mint
    #[account(
        address = token_vault_0.mint
    )]
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    /// token vault 1 的 mint
    #[account(
        address = token_vault_1.mint
    )]
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,
    // ======== IMPORTANT: remaining accounts for swap_v2 =========
    // MUST BE: [bitmap_extension?] + [swap tick arrays ONLY]
    // open_position tick arrays MUST NOT be here!
    //
    // 所有 swap_v2 tick arrays 都在这里动态提供（前端传入）
    //
    // 例如：
    // [
    //   bitmap_extension?,
    //   swap_tick_array_0,
    //   swap_tick_array_1,
    //   swap_tick_array_2,
    // ]
    //
    // open_position 的 tick_array_lower / upper 由上面两个字段指定，不混在这里
    //
}

crate::impl_zap_common_accounts!(IncreaseLiquidity<'info>);

/// 先调用 Raydium `swap_v2` 换币，再调用 `increase_liquidity_v2` 为已有 position 增加流动性
/// 使用 Raydium 的 liquidity_math 精确计算最优 swap 比例
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
    let plan = zap_common::prepare_zap_plan_and_swap_if_needed(
        &mut *ctx.accounts,
        ctx.remaining_accounts,
        amount_0_in,
        amount_1_in,
        return_mint,
        tick_lower_index,
        tick_upper_index,
        slippage_bps,
        swap_amount_in,
        swap_min_out,
        swap_input_is_token0,
    )?;

    increase_liquidity_v2(
        &ctx,
        plan.computed_liquidity,
        plan.amount_0_max,
        plan.amount_1_max,
        plan.base_flag, // base_flag
        plan.action_remaining,
    )?;

    // 先取出 mint，避免后续 `&mut *ctx.accounts` 借用期间再借用 ctx.accounts
    let position_nft_mint = ctx.accounts.position_nft_account.mint;

    zap_common::swap_back_remaining_and_emit_increase_event(
        &mut *ctx.accounts,
        amount_0_in,
        amount_1_in,
        return_mint,
        tick_lower_index,
        tick_upper_index,
        plan.computed_liquidity,
        slippage_bps,
        plan.balance_0_pre_cpi,
        plan.balance_1_pre_cpi,
        plan.amount_0_max,
        plan.amount_1_max,
        plan.swap_remaining,
        position_nft_mint,
    )?;

    Ok(())
}

fn increase_liquidity_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, IncreaseLiquidity<'info>>,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
    increase_liquidity_remaining: &[AccountInfo<'info>],
) -> Result<()> {
    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();

    // 使用解构简化代码
    let accounts = &ctx.accounts;
    let cpi_accounts = clmm_accounts::IncreaseLiquidityV2 {
        nft_owner: accounts.signer.to_account_info(),
        nft_account: accounts.position_nft_account.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        token_account_0: accounts.signer_token0_account.to_account_info(),
        token_account_1: accounts.signer_token1_account.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts)
        .with_remaining_accounts(increase_liquidity_remaining.to_vec());

    clmm_cpi::increase_liquidity_v2(cpi_ctx, liquidity, amount_0_max, amount_1_max, base_flag)?;

    Ok(())
}
