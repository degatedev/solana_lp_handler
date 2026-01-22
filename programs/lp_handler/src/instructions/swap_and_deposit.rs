use anchor_lang::prelude::*;
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{AmmConfig, ObservationState, PoolState, TickArrayState};

use crate::{utils, LpDepositError, SECURITY_CONFIG_SEED};

use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

use super::zap_common;

/// swap_and_deposit 所需的所有账户
/// 包含 swap_v2 和 open_position_v2 的全部账户（有些可以复用，比如 pool_state、token_program 等）
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
pub struct SwapAndDeposit<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program (主网: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK)
    /// CHECK: 地址已通过 `#[account(address = ...)]` 约束为 Raydium CLMM programId
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// 支付者 / 签名者
    #[account(mut)]
    pub user: Signer<'info>,

    /// AMM 配置账户（swap 和 position 都需要通过 pool_state 关联）
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool 状态账户（swap 和 open_position 都需要）
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Observation 状态（swap 需要）
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    #[account(
        mut,
        token::mint = token_vault_0.mint,
    )]
    pub user_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
    )]
    pub user_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: position NFT 的接收者（owner）。安全层会校验其 authority 关系
    pub position_nft_owner: UncheckedAccount<'info>,

    /// position NFT 的 mint（由调用方提供并签名；CPI 会创建并 mint）
    #[account(mut)]
    pub position_nft_mint: Signer<'info>,

    /// CHECK: position NFT 将被 mint 到的 token account（通常是 ATA；本指令只校验地址）
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: `protocol_position` 已废弃，仅为兼容保留
    pub protocol_position: UncheckedAccount<'info>,

    /// CHECK: Raydium CLMM 使用的 TickArray PDA；CPI 时由 Raydium 侧校验/派生
    #[account(mut)]
    pub tick_array_lower: UncheckedAccount<'info>,

    /// CHECK: Raydium CLMM 使用的 TickArray PDA；CPI 时由 Raydium 侧校验/派生
    #[account(mut)]
    pub tick_array_upper: UncheckedAccount<'info>,

    pub memo_program: Program<'info, Memo>,

    /// CHECK: PersonalPosition 状态账户；由 Raydium CLMM 程序在 CPI 中校验
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

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

    /// ATA 程序（用于创建接收 position NFT 的 ATA）
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

crate::impl_zap_common_accounts!(SwapAndDeposit<'info>);

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
    // 校验 position_nft_account 必须是 (position_nft_owner, position_nft_mint, Token2022) 的 ATA 地址
    // 注意：该 ATA 可能尚未初始化（由下游 CPI 创建），因此只校验地址本身，不校验 owner/program。
    let expected_position_nft_ata = utils::derive_ata_address(
        &ctx.accounts.position_nft_owner.key(),
        &ctx.accounts.position_nft_mint.key(),
        &ctx.accounts.token_program_2022.key(),
        &ctx.accounts.associated_token_program.key(),
    );
    require_keys_eq!(
        ctx.accounts.position_nft_account.key(),
        expected_position_nft_ata,
        LpDepositError::InvalidPositionNftAccount
    );

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

    let tick_array_lower_start_index =
        TickArrayState::get_array_start_index(tick_lower_index, plan.tick_spacing);
    let tick_array_upper_start_index =
        TickArrayState::get_array_start_index(tick_upper_index, plan.tick_spacing);
    // 7. 添加流动性
    // 注意：这里固定传 0 是“刻意设计”
    // - `liquidity` 参数仅用于上面的 `calculate_optimal_swap_amount` 计算最优兑换比例
    // - 开仓/加流动性时让 Raydium 根据 amount_0_max/amount_1_max 自动计算实际 liquidity

    open_position_with_token22_nft(
        &ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
        plan.computed_liquidity,
        plan.amount_0_max,
        plan.amount_1_max,
        plan.base_flag, // base_flag
        plan.action_remaining,
    )?;

    let position_nft_mint = ctx.accounts.position_nft_mint.key();
    let _return_amount = zap_common::swap_back_remaining_and_emit_increase_event(
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
        Some(position_nft_mint),
    )?;

    Ok(())
}

fn open_position_with_token22_nft<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, SwapAndDeposit<'info>>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    tick_array_lower_start_index: i32,
    tick_array_upper_start_index: i32,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
    open_position_remaining: Vec<AccountInfo<'info>>,
) -> Result<()> {
    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();

    // 使用解构简化代码
    let accounts = &ctx.accounts;
    let cpi_accounts = clmm_accounts::OpenPositionWithToken22Nft {
        payer: accounts.user.to_account_info(),
        position_nft_owner: accounts.position_nft_owner.to_account_info(),
        position_nft_mint: accounts.position_nft_mint.to_account_info(),
        position_nft_account: accounts.position_nft_account.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        token_account_0: accounts.user_token0_account.to_account_info(),
        token_account_1: accounts.user_token1_account.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        rent: accounts.rent.to_account_info(),
        system_program: accounts.system_program.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        associated_token_program: accounts.associated_token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx =
        CpiContext::new(cpi_program, cpi_accounts).with_remaining_accounts(open_position_remaining);

    clmm_cpi::open_position_with_token22_nft(
        cpi_ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
        liquidity,
        amount_0_max,
        amount_1_max,
        true,
        base_flag,
    )?;

    Ok(())
}
