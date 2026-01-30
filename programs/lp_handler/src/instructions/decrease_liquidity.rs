use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_amm_v3::program::AmmV3;

use super::zap_common;
use crate::{utils, LpDepositError, LpHandlerDecreaseLiquidityEvent, SECURITY_CONFIG_SEED};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::states::{
    AmmConfig, ObservationState, PersonalPositionState, PoolState, TickArrayState,
};

use crate::require_log;

#[derive(Accounts)]
#[instruction(
  liquidity:u128,
  mint_amount_0:u64,
  mint_amount_1:u64,
  swap_to_token_mint: Pubkey,
  slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
  fee_percent: u16,
  convert_to_usdc: bool,
)]
pub struct DecreaseLiquidity<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program (主网: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK)
    /// CHECK: 地址已通过 `#[account(address = ...)]` 约束为 Raydium CLMM programId
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// CHECK: withdraw 和 claim 的接收者 必须定义到第二个账户，前端需要校验recipient 地址
    #[account(mut)]
    pub recipient: UncheckedAccount<'info>,

    /// 支付者 / 签名者
    #[account(mut)]
    pub signer: Signer<'info>,

    /// signer 的 token0 TokenAccount（中转账户，swap/扣费都从这里扣）
    #[account(
        mut,
        token::mint = token_vault_0.mint,
        token::authority = signer,
    )]
    pub signer_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// signer 的 token1 TokenAccount（中转账户，swap/扣费都从这里扣）
    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = signer,
    )]
    pub signer_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// recipient 的 token0 TokenAccount（通常为 ATA；需前端/调用方确保已创建，或在同笔交易里先创建）
    #[account(
        mut,
        token::mint = token_vault_0.mint,
        token::authority = recipient,
      )]
    pub recipient_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// recipient 的 token1 TokenAccount（通常为 ATA；需前端/调用方确保已创建，或在同笔交易里先创建）
    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = recipient,
      )]
    pub recipient_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 固定 integrator fee 收款人（防止用户把 fee 转回自己绕过抽成）
    // 这里不能用 `#[account(address = ...)]` 写死单一地址，因为我们支持多个固定收款人白名单；
    // 在 handler 内做运行时校验（见下方 require!）。
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

    /// AMM 配置账户（swap 和 position 都需要通过 pool_state 关联）
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool 状态账户（swap 需要）
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Observation 状态（swap 需要）
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// Rent sysvar（用于 mint/ATA 创建等租金相关逻辑）
    pub rent: Sysvar<'info, Rent>,

    /// 系统程序（创建/分配账户）
    pub system_program: Program<'info, System>,

    /// SPL Token 程序（Token-2022 之外的转账等）
    pub token_program: Program<'info, Token>,

    /// ATA 程序（用于创建/校验 token account 地址）
    pub associated_token_program: Program<'info, AssociatedToken>,

    /// Token-2022 程序（用于 position NFT 的 token account/转账/close 等）
    pub token_program_2022: Program<'info, Token2022>,

    /// CHECK: 通用安全配置 PDA（必须传入），允许“未初始化”的 system-owned 空账户。
    /// - 地址通过 seeds+bump 校验为固定 PDA
    /// - 是否初始化由安全层在运行时判断；未初始化时跳过 pool 白名单校验
    #[account(
        seeds = [SECURITY_CONFIG_SEED],
        bump
    )]
    pub security_config: UncheckedAccount<'info>,

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

    /// CHECK: position NFT 的 token account（通常是 ATA；用于验证/关闭 position NFT）
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: `protocol_position` 已废弃，仅为兼容保留
    pub protocol_position: UncheckedAccount<'info>,

    /// CHECK: PersonalPosition 状态账户；由 Raydium CLMM 程序在 CPI 中校验
    #[account(mut, constraint = personal_position.pool_id == pool_state.key())]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// lower tick 对应的 TickArray 状态（由 Raydium 使用）
    #[account(mut, constraint = tick_array_lower.load()?.pool_id == pool_state.key())]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// upper tick 对应的 TickArray 状态（由 Raydium 使用）
    #[account(mut, constraint = tick_array_upper.load()?.pool_id == pool_state.key())]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,
    // ======== IMPORTANT: remaining accounts for swap_v2 =========
    // MUST BE: [bitmap_extension?] + [swap tick arrays ONLY]
    // 所有 swap_v2 tick arrays 都在这里动态提供（前端传入）
    // 例如：
    // [
    //   bitmap_extension?,
    //   swap_tick_array_0,
    //   swap_tick_array_1,
    //   swap_tick_array_2,
    // ]
    //
    //
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
    // 校验手续费比例，最大 100%（10000 bps）
    require_log!(
        fee_percent <= 10_000,
        LpDepositError::InvalidFeePercent,
        "fee_percent={}",
        fee_percent
    );
    require_log!(
        slippage_bps <= 5_000,
        LpDepositError::InvalidSlippage,
        "slippage_bps={}",
        slippage_bps
    );
    require_log!(
        swap_to_token_mint == ctx.accounts.vault_0_mint.key()
            || swap_to_token_mint == ctx.accounts.vault_1_mint.key(),
        LpDepositError::InvalidDepositMint,
        "swap_to_token_mint={}",
        swap_to_token_mint
    );

    // -----------------------------------
    // BEFORE: 读取 signer 中转账户余额（用于余额差计算，避免混用 recipient authority）
    // -----------------------------------
    let signer_token0_balance_before = ctx.accounts.signer_token0_account.amount;
    let signer_token1_balance_before = ctx.accounts.signer_token1_account.amount;

    let sep = crate::ID; // 你的 lp_handler program id（分隔符）

    let sep_index = ctx
        .remaining_accounts
        .iter()
        .position(|a| a.key() == sep)
        .ok_or(LpDepositError::InvalidRemainingAccounts)?;

    let (swap_remaining, rest) = ctx.remaining_accounts.split_at(sep_index);
    let decrease_remaining = &rest[1..]; // 跳过分隔符本身

    // -----------------------------
    // 关键：用“余额增量 - principal”得到奖励/手续费，再只对奖励/手续费抽成
    // - claim: liquidity=0 => principal=0 => 全部增量都视为奖励/手续费
    // - withdraw: liquidity>0 => principal>0 => 奖励/手续费 = 增量 - principal
    // -----------------------------
    let (principal_expected_0, principal_expected_1) = if liquidity == 0 {
        (0u64, 0u64)
    } else {
        let (tick_current_before, sqrt_price_x64_before) = {
            let pool_state = ctx.accounts.pool_state.load()?;
            (pool_state.tick_current, pool_state.sqrt_price_x64)
        };

        utils::calculate_principal_amounts_for_liquidity(
            tick_current_before,
            sqrt_price_x64_before,
            ctx.accounts.personal_position.tick_lower_index,
            ctx.accounts.personal_position.tick_upper_index,
            liquidity,
        )?
    };
    cpi_decrease_liquidity_v2(
        &ctx,
        liquidity,
        mint_amount_0,
        mint_amount_1,
        decrease_remaining,
    )?;

    ctx.accounts.signer_token0_account.reload()?;
    ctx.accounts.signer_token1_account.reload()?;
    ctx.accounts.personal_position.reload()?;

    let signer_token0_balance_after = ctx.accounts.signer_token0_account.amount;
    let signer_token1_balance_after = ctx.accounts.signer_token1_account.amount;

    let signer_token0_amount = signer_token0_balance_after
        .checked_sub(signer_token0_balance_before)
        .ok_or(LpDepositError::MathOverflow)?;
    let signer_token1_amount = signer_token1_balance_after
        .checked_sub(signer_token1_balance_before)
        .ok_or(LpDepositError::MathOverflow)?;

    // 所有情况：如果本次操作没有带来任何余额变化（两边增量都为 0），直接失败
    require_log!(
        signer_token0_amount > 0 || signer_token1_amount > 0,
        LpDepositError::NoBalanceChange
    );

    let reward_gross_0 = signer_token0_amount.saturating_sub(principal_expected_0);
    let reward_gross_1 = signer_token1_amount.saturating_sub(principal_expected_1);

    // wSOL 手续费要求：
    // - 如果手续费币种是 wSOL(native mint)，则手续费必须以 SOL(lamports) 形式转给 fee_owner。
    // - 这里先记录“应扣手续费金额（按 token 最小单位）”，最终在结算阶段统一 unwrap -> system transfer 支付。
    let mut fee_due_0: u64 = 0;
    let mut fee_due_1: u64 = 0;

    if convert_to_usdc {
        // 兑换到目标币种后再扣手续费（手续费从“最终到手的 reward”中抽取，且用目标币种结算）
        let target_is_token0 = ctx.accounts.vault_0_mint.key() == swap_to_token_mint;
        // 合并两段 swap，把“other token”一次性换成目标币种；
        // 然后按 (reward_other_in / total_other_in) 的比例把 swap 输出近似拆分为 reward/principal，
        // fee 仍然只对 reward 口径计提。
        // 注意：这里的 swap 输入来自 decrease_liquidity_v2 后的增量（user_token*_amount），不会动到用户原有余额。
        let (reward_other_in, principal_other_in, reward_target_direct) = if target_is_token0 {
            // token1 -> token0
            (
                reward_gross_1,
                signer_token1_amount
                    .checked_sub(reward_gross_1)
                    .unwrap_or(0),
                reward_gross_0,
            )
        } else {
            // token0 -> token1
            (
                reward_gross_0,
                signer_token0_amount
                    .checked_sub(reward_gross_0)
                    .unwrap_or(0),
                reward_gross_1,
            )
        };

        let input_is_token0 = !target_is_token0; // 目标是 token0 => 输入 token1；目标是 token1 => 输入 token0

        // 记录兑换前目标币种余额，用于计算 swap 的实际输出
        let target_balance_before_swap = if target_is_token0 {
            ctx.accounts.signer_token0_account.amount
        } else {
            ctx.accounts.signer_token1_account.amount
        };

        // 1) 合并 swap：把 other token 的增量一次性兑换成目标币种（若 total_other_in == 0 则不 swap）
        let total_other_in = reward_other_in
            .checked_add(principal_other_in)
            .ok_or(LpDepositError::MathOverflow)?;
        let mut total_out_in_target: u64 = 0;
        if total_other_in > 0 {
            // 为了避免“用旧价格估 min_out”导致过严/过松，在 CPI swap_v2 前重新读取 pool_state 的最新价格。
            let sqrt_price_x64_for_min_out = {
                let pool_state = ctx.accounts.pool_state.load()?;
                pool_state.sqrt_price_x64
            };
            let swap_other_amount_threshold = utils::calc_min_amount_out(
                total_other_in,
                input_is_token0,
                sqrt_price_x64_for_min_out,
                slippage_bps,
                ctx.accounts.amm_config.trade_fee_rate,
            )?;

            // dust 处理：
            // - 合并 swap 后，若同时包含 principal，则不能把输入直接转给 fee（会误伤本金）
            // - 但在“纯领取奖励”（principal_other_in==0）场景下，可以保留原逻辑：当 min_out==0 时直接把 reward_other_in 转给 fee
            if principal_other_in == 0 && swap_other_amount_threshold == 0 {
                msg!("skip reward swap (claim-only dust): amount_out_min=0, transfer input to fee");
                zap_common::transfer_fee(
                    &ctx.accounts.signer,
                    &ctx.accounts.fee_owner,
                    if input_is_token0 {
                        &ctx.accounts.signer_token0_account
                    } else {
                        &ctx.accounts.signer_token1_account
                    },
                    if input_is_token0 {
                        &ctx.accounts.fee_token0_account
                    } else {
                        &ctx.accounts.fee_token1_account
                    },
                    if input_is_token0 {
                        Some(&ctx.accounts.vault_0_mint)
                    } else {
                        Some(&ctx.accounts.vault_1_mint)
                    },
                    &ctx.accounts.token_program,
                    Some(&ctx.accounts.token_program_2022),
                    &ctx.accounts.system_program,
                    total_other_in, // == reward_other_in
                )?;
            } else {
                swap_v2(
                    &ctx,
                    total_other_in,
                    swap_other_amount_threshold,
                    0,
                    input_is_token0,
                    swap_remaining.to_vec(),
                )?;
                ctx.accounts.signer_token0_account.reload()?;
                ctx.accounts.signer_token1_account.reload()?;
                let target_balance_after_swap = if target_is_token0 {
                    ctx.accounts.signer_token0_account.amount
                } else {
                    ctx.accounts.signer_token1_account.amount
                };
                total_out_in_target = target_balance_after_swap
                    .checked_sub(target_balance_before_swap)
                    .ok_or(LpDepositError::MathOverflow)?;
            }
        }

        // 2) 近似拆分 swap 输出：reward_out ≈ total_out * reward_other_in / total_other_in
        let reward_out_in_target_est = if total_other_in == 0 || reward_other_in == 0 {
            0u64
        } else {
            let num = (total_out_in_target as u128)
                .checked_mul(reward_other_in as u128)
                .ok_or(LpDepositError::MathOverflow)?;
            let den = total_other_in as u128;
            (num / den) as u64
        };
        let principal_out_in_target_est = total_out_in_target
            .checked_sub(reward_out_in_target_est)
            .ok_or(LpDepositError::MathOverflow)?;

        // 3) fee 仍按 reward 口径计提（reward_direct + reward_out_est）
        let reward_total_in_target = reward_target_direct
            .checked_add(reward_out_in_target_est)
            .ok_or(LpDepositError::MathOverflow)?;
        let integrator_fee_target = reward_total_in_target
            .checked_mul(fee_percent as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(LpDepositError::MathOverflow)?;

        if target_is_token0 {
            fee_due_0 = integrator_fee_target;
        } else {
            fee_due_1 = integrator_fee_target;
        }

        // 非 wSOL 手续费：可以直接扣 token 给 fee_token_account。
        // wSOL 手续费：必须收 SOL，因此不在这里扣，留到末尾统一 unwrap 后用 system transfer 支付。
        let target_is_wsol = if target_is_token0 {
            ctx.accounts.vault_0_mint.key() == anchor_spl::token::spl_token::native_mint::ID
        } else {
            ctx.accounts.vault_1_mint.key() == anchor_spl::token::spl_token::native_mint::ID
        };
        if !target_is_wsol {
            zap_common::transfer_fee(
                &ctx.accounts.signer,
                &ctx.accounts.fee_owner,
                if target_is_token0 {
                    &ctx.accounts.signer_token0_account
                } else {
                    &ctx.accounts.signer_token1_account
                },
                if target_is_token0 {
                    &ctx.accounts.fee_token0_account
                } else {
                    &ctx.accounts.fee_token1_account
                },
                if target_is_token0 {
                    Some(&ctx.accounts.vault_0_mint)
                } else {
                    Some(&ctx.accounts.vault_1_mint)
                },
                &ctx.accounts.token_program,
                Some(&ctx.accounts.token_program_2022),
                &ctx.accounts.system_program,
                integrator_fee_target,
            )?;
        }

        // 事件按“兑换后”口径输出：只在目标币种上体现 principal/reward/fee，其它币种为 0
        if target_is_token0 {
            let principal_amount_0 = principal_expected_0
                .checked_add(principal_out_in_target_est)
                .ok_or(LpDepositError::MathOverflow)?;
            let reward_amount_0 = reward_total_in_target
                .checked_sub(integrator_fee_target)
                .ok_or(LpDepositError::MathOverflow)?;
            emit!(LpHandlerDecreaseLiquidityEvent {
                pool: ctx.accounts.pool_state.key(),
                token0_mint: ctx.accounts.vault_0_mint.key(),
                token1_mint: ctx.accounts.vault_1_mint.key(),
                settle_mint: Some(swap_to_token_mint),
                principal_pre_0: principal_expected_0,
                principal_pre_1: principal_expected_1,
                reward_pre_fee_0: reward_gross_0,
                reward_pre_fee_1: reward_gross_1,
                principal_settled_0: principal_amount_0,
                principal_settled_1: 0,
                reward_settled_0: reward_amount_0,
                reward_settled_1: 0,
                fee_settled_0: integrator_fee_target,
                fee_settled_1: 0,
            });
        } else {
            let principal_amount_1 = principal_expected_1
                .checked_add(principal_out_in_target_est)
                .ok_or(LpDepositError::MathOverflow)?;
            let reward_amount_1 = reward_total_in_target
                .checked_sub(integrator_fee_target)
                .ok_or(LpDepositError::MathOverflow)?;
            emit!(LpHandlerDecreaseLiquidityEvent {
                pool: ctx.accounts.pool_state.key(),
                token0_mint: ctx.accounts.vault_0_mint.key(),
                token1_mint: ctx.accounts.vault_1_mint.key(),
                settle_mint: Some(swap_to_token_mint),
                principal_pre_0: principal_expected_0,
                principal_pre_1: principal_expected_1,
                reward_pre_fee_0: reward_gross_0,
                reward_pre_fee_1: reward_gross_1,
                principal_settled_0: 0,
                principal_settled_1: principal_amount_1,
                reward_settled_0: 0,
                reward_settled_1: reward_amount_1,
                fee_settled_0: 0,
                fee_settled_1: integrator_fee_target,
            });
        }
        // wSOL unwrap / 手续费支付 / 转给 recipient 统一在函数末尾结算阶段处理。
    } else {
        // 不换币：直接对两种 token 的 reward 部分分别抽成
        let integrator_fee_0 = reward_gross_0
            .checked_mul(fee_percent as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(LpDepositError::MathOverflow)?;
        let integrator_fee_1 = reward_gross_1
            .checked_mul(fee_percent as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(LpDepositError::MathOverflow)?;
        fee_due_0 = integrator_fee_0;
        fee_due_1 = integrator_fee_1;

        let is_wsol_0 =
            ctx.accounts.vault_0_mint.key() == anchor_spl::token::spl_token::native_mint::ID;
        let is_wsol_1 =
            ctx.accounts.vault_1_mint.key() == anchor_spl::token::spl_token::native_mint::ID;

        // 非 wSOL 手续费：直接扣 token 给 fee_token_account。
        // wSOL 手续费：必须收 SOL，因此不在这里扣，留到末尾统一 unwrap 后用 system transfer 支付。
        if !is_wsol_0 {
            zap_common::transfer_fee(
                &ctx.accounts.signer,
                &ctx.accounts.fee_owner,
                &ctx.accounts.signer_token0_account,
                &ctx.accounts.fee_token0_account,
                Some(&ctx.accounts.vault_0_mint),
                &ctx.accounts.token_program,
                Some(&ctx.accounts.token_program_2022),
                &ctx.accounts.system_program,
                integrator_fee_0,
            )?;
        }

        if !is_wsol_1 {
            zap_common::transfer_fee(
                &ctx.accounts.signer,
                &ctx.accounts.fee_owner,
                &ctx.accounts.signer_token1_account,
                &ctx.accounts.fee_token1_account,
                Some(&ctx.accounts.vault_1_mint),
                &ctx.accounts.token_program,
                Some(&ctx.accounts.token_program_2022),
                &ctx.accounts.system_program,
                integrator_fee_1,
            )?;
        }

        let principal_amount_0 = principal_expected_0;
        let principal_amount_1 = principal_expected_1;
        // 事件字段仍按“reward 口径 - fee”展示（与实际扣费一致）
        let reward_amount_0 = reward_gross_0
            .checked_sub(integrator_fee_0)
            .ok_or(LpDepositError::MathOverflow)?;
        let reward_amount_1 = reward_gross_1
            .checked_sub(integrator_fee_1)
            .ok_or(LpDepositError::MathOverflow)?;
        emit!(LpHandlerDecreaseLiquidityEvent {
            pool: ctx.accounts.pool_state.key(),
            token0_mint: ctx.accounts.vault_0_mint.key(),
            token1_mint: ctx.accounts.vault_1_mint.key(),
            settle_mint: None,
            principal_pre_0: principal_expected_0,
            principal_pre_1: principal_expected_1,
            reward_pre_fee_0: reward_gross_0,
            reward_pre_fee_1: reward_gross_1,
            principal_settled_0: principal_amount_0,
            principal_settled_1: principal_amount_1,
            reward_settled_0: reward_amount_0,
            reward_settled_1: reward_amount_1,
            fee_settled_0: integrator_fee_0,
            fee_settled_1: integrator_fee_1,
        });
    }

    // 最后：统一结算转给 recipient（若 recipient==signer 则跳过），并处理 wSOL 的 unwrap 与 SOL 手续费支付。
    ctx.accounts.signer_token0_account.reload()?;
    ctx.accounts.signer_token1_account.reload()?;
    let net0 = ctx
        .accounts
        .signer_token0_account
        .amount
        .checked_sub(signer_token0_balance_before)
        .unwrap_or(0);
    let net1 = ctx
        .accounts
        .signer_token1_account
        .amount
        .checked_sub(signer_token1_balance_before)
        .unwrap_or(0);

    let recipient_is_signer = ctx.accounts.recipient.key() == ctx.accounts.signer.key();
    let is_wsol_0 =
        ctx.accounts.vault_0_mint.key() == anchor_spl::token::spl_token::native_mint::ID;
    let is_wsol_1 =
        ctx.accounts.vault_1_mint.key() == anchor_spl::token::spl_token::native_mint::ID;

    // token0 结算
    if net0 > 0 {
        if is_wsol_0 {
            require_log!(net0 >= fee_due_0, LpDepositError::MathOverflow);

            // unwrap：close wSOL token account，把 lamports 退回 signer
            zap_common::unwrap_wsol_to_destination(
                ctx.accounts.signer.to_account_info(),
                ctx.accounts.signer_token0_account.to_account_info(),
                ctx.accounts.signer.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
                Some(ctx.accounts.token_program_2022.to_account_info()),
            )?;

            // fee：以 SOL(lamports) 支付给 fee_owner
            if fee_due_0 > 0 {
                anchor_lang::system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        anchor_lang::system_program::Transfer {
                            from: ctx.accounts.signer.to_account_info(),
                            to: ctx.accounts.fee_owner.to_account_info(),
                        },
                    ),
                    fee_due_0,
                )?;
            }

            // remainder：给 recipient（若 recipient != signer）
            let rem = net0
                .checked_sub(fee_due_0)
                .ok_or(LpDepositError::MathOverflow)?;
            if !recipient_is_signer && rem > 0 {
                anchor_lang::system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        anchor_lang::system_program::Transfer {
                            from: ctx.accounts.signer.to_account_info(),
                            to: ctx.accounts.recipient.to_account_info(),
                        },
                    ),
                    rem,
                )?;
            }
        } else if !recipient_is_signer {
            zap_common::transfer_token_to_fee_accounts(
                ctx.accounts.signer.to_account_info(),
                ctx.accounts.signer_token0_account.to_account_info(),
                ctx.accounts.recipient_token0_account.to_account_info(),
                ctx.accounts.vault_0_mint.to_account_info(),
                ctx.accounts.vault_0_mint.decimals,
                ctx.accounts.token_program.to_account_info(),
                Some(ctx.accounts.token_program_2022.to_account_info()),
                net0,
            )?;
        }
    }

    // token1 结算
    if net1 > 0 {
        if is_wsol_1 {
            require_log!(net1 >= fee_due_1, LpDepositError::MathOverflow);

            zap_common::unwrap_wsol_to_destination(
                ctx.accounts.signer.to_account_info(),
                ctx.accounts.signer_token1_account.to_account_info(),
                ctx.accounts.signer.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
                Some(ctx.accounts.token_program_2022.to_account_info()),
            )?;

            if fee_due_1 > 0 {
                anchor_lang::system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        anchor_lang::system_program::Transfer {
                            from: ctx.accounts.signer.to_account_info(),
                            to: ctx.accounts.fee_owner.to_account_info(),
                        },
                    ),
                    fee_due_1,
                )?;
            }

            let rem = net1
                .checked_sub(fee_due_1)
                .ok_or(LpDepositError::MathOverflow)?;
            if !recipient_is_signer && rem > 0 {
                anchor_lang::system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        anchor_lang::system_program::Transfer {
                            from: ctx.accounts.signer.to_account_info(),
                            to: ctx.accounts.recipient.to_account_info(),
                        },
                    ),
                    rem,
                )?;
            }
        } else if !recipient_is_signer {
            zap_common::transfer_token_to_fee_accounts(
                ctx.accounts.signer.to_account_info(),
                ctx.accounts.signer_token1_account.to_account_info(),
                ctx.accounts.recipient_token1_account.to_account_info(),
                ctx.accounts.vault_1_mint.to_account_info(),
                ctx.accounts.vault_1_mint.decimals,
                ctx.accounts.token_program.to_account_info(),
                Some(ctx.accounts.token_program_2022.to_account_info()),
                net1,
            )?;
        }
    }

    Ok(())
}

/// 重要：把 Raydium CPI 的 accounts struct 构造移出主 handler，避免 BPF 4KB 栈帧超限。
#[inline(never)]
fn cpi_decrease_liquidity_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
    liquidity: u128,
    mint_amount_0: u64,
    mint_amount_1: u64,
    decrease_remaining: &[AccountInfo<'info>],
) -> Result<()> {
    let accounts = &ctx.accounts;
    let cpi_program = accounts.raydium_clmm_program.to_account_info();
    let cpi_accounts = clmm_accounts::DecreaseLiquidityV2 {
        nft_owner: accounts.signer.to_account_info(),
        nft_account: accounts.position_nft_account.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        // 输出先打到 signer 的中转账户，后续 swap/扣费都从 signer authority 执行
        recipient_token_account_0: accounts.signer_token0_account.to_account_info(),
        recipient_token_account_1: accounts.signer_token1_account.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        memo_program: accounts.memo_program.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts)
        .with_remaining_accounts(decrease_remaining.to_vec());
    clmm_cpi::decrease_liquidity_v2(cpi_ctx, liquidity, mint_amount_0, mint_amount_1)
}

#[inline(never)]
fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
    swap_amount: u64,
    swap_other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_token0: bool,
    swap_remaining: Vec<AccountInfo<'info>>,
) -> Result<()> {
    // 将 AccountInfo 的构造放到单独函数，避免增大主 handler 栈帧（BPF 栈限制 4KB）。
    let accounts = &ctx.accounts;

    let (input_token, output_token, input_vault, output_vault, input_mint, output_mint) =
        if is_token0 {
            (
                accounts.signer_token0_account.to_account_info(),
                accounts.signer_token1_account.to_account_info(),
                accounts.token_vault_0.to_account_info(),
                accounts.token_vault_1.to_account_info(),
                accounts.vault_0_mint.to_account_info(),
                accounts.vault_1_mint.to_account_info(),
            )
        } else {
            (
                accounts.signer_token1_account.to_account_info(),
                accounts.signer_token0_account.to_account_info(),
                accounts.token_vault_1.to_account_info(),
                accounts.token_vault_0.to_account_info(),
                accounts.vault_1_mint.to_account_info(),
                accounts.vault_0_mint.to_account_info(),
            )
        };

    zap_common::swap_v2_accounts(
        accounts.raydium_clmm_program.to_account_info(),
        accounts.signer.to_account_info(),
        accounts.amm_config.to_account_info(),
        accounts.pool_state.to_account_info(),
        accounts.observation_state.to_account_info(),
        accounts.token_program.to_account_info(),
        accounts.token_program_2022.to_account_info(),
        accounts.memo_program.to_account_info(),
        input_token,
        output_token,
        input_vault,
        output_vault,
        input_mint,
        output_mint,
        swap_remaining,
        swap_amount,
        swap_other_amount_threshold,
        sqrt_price_limit_x64,
    )
}
