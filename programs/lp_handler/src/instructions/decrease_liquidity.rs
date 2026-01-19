use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token::{self, Token};
use anchor_spl::token_2022::{self, Token2022};
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_amm_v3::program::AmmV3;

use crate::{is_fee_owner, utils, DecreaseLiquidityEvent, LpDepositError};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::states::{
    AmmConfig, ObservationState, PersonalPositionState, PoolState, TickArrayState,
};

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
    /// CHECK: 前端传入 Raydium CLMM programId
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// 支付者 / 签名者
    #[account(mut)]
    pub user: Signer<'info>,

    /// user 的 token0 ATA 账户（如果不存在则自动创建）
    #[account(
        mut,
        token::mint = token_vault_0.mint,
      )]
    pub user_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// user 的 token1 ATA 账户（如果不存在则自动创建）
    #[account(
        mut,
        token::mint = token_vault_1.mint,
      )]
    pub user_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 固定 integrator fee 收款人（防止用户把 fee 转回自己绕过抽成）
    // 这里不能用 `#[account(address = ...)]` 写死单一地址，因为我们支持多个固定收款人白名单；
    // 在 handler 内做运行时校验（见下方 require!）。
    #[account(mut)]
    pub fee_owner: SystemAccount<'info>,

    #[account(mut)]
    pub fee_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub fee_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// AMM 配置账户（swap 和 position 都需要通过 pool_state 关联）
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool 状态账户（swap 和 open_position 都需要）
    #[account(
        mut,
        constraint = pool_state.load()?.amm_config == amm_config.key()
    )]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Observation 状态（swap 需要）
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// Sysvar for token mint and ATA creation
    pub rent: Sysvar<'info, Rent>,

    /// Program to create the position manager state account
    pub system_program: Program<'info, System>,

    /// Program to transfer for token account
    pub token_program: Program<'info, Token>,

    /// Program to create an ATA for receiving position NFT
    pub associated_token_program: Program<'info, AssociatedToken>,

    /// Program to create NFT mint/token account and transfer for token22 account
    pub token_program_2022: Program<'info, Token2022>,

    pub memo_program: Program<'info, Memo>,

    /// The address that holds pool tokens for token_0
    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The address that holds pool tokens for token_1
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The mint of token vault 0
    #[account(
      address = token_vault_0.mint
    )]
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of token vault 1
    #[account(
      address = token_vault_1.mint
    )]
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Unique token mint address, initialize in contract

    /// CHECK: ATA address where position NFT will be minted, initialize in contract
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: Deprecated: protocol_position is deprecated and kept for compatibility.
    pub protocol_position: UncheckedAccount<'info>,

    /// CHECK: Personal position state account, validated by Raydium CLMM program
    #[account(mut, constraint = personal_position.pool_id == pool_state.key())]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// Stores init state for the lower tick
    #[account(mut, constraint = tick_array_lower.load()?.pool_id == pool_state.key())]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// Stores init state for the upper tick
    #[account(mut, constraint = tick_array_upper.load()?.pool_id == pool_state.key())]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,
    // ======== IMPORTANT: remaining accounts for swap_v2 =========
    // MUST BE: [bitmap_extension?] + [swap tick arrays ONLY]
    // 所有 swap_v2 tick arrays 都在这里动态提供（前端传入）

    // 比如这里我要swap remaining accounts 和 decrease_liquidity_v2 的remaining accounts 用两个集合接收

    //
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
    let timestamp = Clock::get()?.unix_timestamp;
    // 固定 fee 收款人白名单校验（支持多个固定地址）
    require!(
        is_fee_owner(&ctx.accounts.fee_owner.key()),
        LpDepositError::InvalidFeeOwner
    );
    // 校验手续费比例，最大 100%（10000 bps）
    require!(fee_percent <= 10_000, LpDepositError::InvalidFeePercent);
    require!(slippage_bps <= 10_000, LpDepositError::InvalidSlippage);
    require!(
        swap_to_token_mint == ctx.accounts.vault_0_mint.key()
            || swap_to_token_mint == ctx.accounts.vault_1_mint.key(),
        LpDepositError::InvalidDepositMint
    );

    // 固定 fee 收款账户：必须是 fee_owner 对应 mint 的 ATA（支持 token / token2022）
    // vault mint 的账户 owner 就是它的 token program（spl-token 或 token-2022）
    let vault0_token_program = ctx.accounts.vault_0_mint.to_account_info().owner;
    let vault1_token_program = ctx.accounts.vault_1_mint.to_account_info().owner;
    let expected_fee_ata_0 = utils::derive_ata_address(
        &ctx.accounts.fee_owner.key(),
        &ctx.accounts.vault_0_mint.key(),
        vault0_token_program,
        &ctx.accounts.associated_token_program.key(),
    );
    let expected_fee_ata_1 = utils::derive_ata_address(
        &ctx.accounts.fee_owner.key(),
        &ctx.accounts.vault_1_mint.key(),
        vault1_token_program,
        &ctx.accounts.associated_token_program.key(),
    );
    require_keys_eq!(
        ctx.accounts.fee_token0_account.key(),
        expected_fee_ata_0,
        LpDepositError::InvalidFeeTokenAccount
    );
    require_keys_eq!(
        ctx.accounts.fee_token1_account.key(),
        expected_fee_ata_1,
        LpDepositError::InvalidFeeTokenAccount
    );
    require_keys_eq!(
        ctx.accounts.fee_token0_account.mint,
        ctx.accounts.vault_0_mint.key(),
        LpDepositError::InvalidFeeTokenAccount
    );
    require_keys_eq!(
        ctx.accounts.fee_token1_account.mint,
        ctx.accounts.vault_1_mint.key(),
        LpDepositError::InvalidFeeTokenAccount
    );
    require_keys_eq!(
        ctx.accounts.fee_token0_account.owner,
        ctx.accounts.fee_owner.key(),
        LpDepositError::InvalidFeeTokenAccount
    );
    require_keys_eq!(
        ctx.accounts.fee_token1_account.owner,
        ctx.accounts.fee_owner.key(),
        LpDepositError::InvalidFeeTokenAccount
    );
    // -----------------------------------
    // BEFORE: 读取用户 Token ATA 余额（用于余额差计算）
    // -----------------------------------
    let user_token0_balance_before = ctx.accounts.user_token0_account.amount;
    let user_token1_balance_before = ctx.accounts.user_token1_account.amount;

    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();
    let accounts = &ctx.accounts;

    let cpi_accounts = clmm_accounts::DecreaseLiquidityV2 {
        nft_owner: accounts.user.to_account_info(),
        nft_account: accounts.position_nft_account.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        recipient_token_account_0: accounts.user_token0_account.to_account_info(),
        recipient_token_account_1: accounts.user_token1_account.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        memo_program: accounts.memo_program.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };
    // let remaining_account = ctx.remaining_accounts.to_vec();
    let sep = crate::ID; // 你的 lp_handler program id（分隔符）

    let sep_index = ctx
        .remaining_accounts
        .iter()
        .position(|a| a.key() == sep)
        .ok_or(LpDepositError::InvalidRemainingAccounts)?;

    let (swap_remaining, rest) = ctx.remaining_accounts.split_at(sep_index);
    let decrease_remaining = &rest[1..]; // 跳过分隔符本身

    let cpi_ctx = CpiContext::new(cpi_program.clone(), cpi_accounts)
        .with_remaining_accounts(decrease_remaining.to_vec());
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
    clmm_cpi::decrease_liquidity_v2(cpi_ctx, liquidity, mint_amount_0, mint_amount_1)?;

    ctx.accounts.user_token0_account.reload()?;
    ctx.accounts.user_token1_account.reload()?;
    ctx.accounts.personal_position.reload()?;

    let user_token0_balance_after = ctx.accounts.user_token0_account.amount;
    let user_token1_balance_after = ctx.accounts.user_token1_account.amount;

    let user_token0_amount = user_token0_balance_after
        .checked_sub(user_token0_balance_before)
        .ok_or(LpDepositError::MathOverflow)?;
    let user_token1_amount = user_token1_balance_after
        .checked_sub(user_token1_balance_before)
        .ok_or(LpDepositError::MathOverflow)?;

    // 所有情况：如果本次操作没有带来任何余额变化（两边增量都为 0），直接失败
    require!(
        user_token0_amount > 0 || user_token1_amount > 0,
        LpDepositError::NoBalanceChange
    );

    let reward_gross_0 = user_token0_amount.saturating_sub(principal_expected_0);
    let reward_gross_1 = user_token1_amount.saturating_sub(principal_expected_1);

    if convert_to_usdc {
        // 兑换到目标币种后再扣手续费（手续费从“最终到手的 reward”中抽取，且用目标币种结算）
        let target_is_token0 = ctx.accounts.vault_0_mint.key() == swap_to_token_mint;
        msg!(
            "target_is_token0: {}, vault_0_mint: {}, swap_to_token_mint: {}",
            target_is_token0,
            ctx.accounts.vault_0_mint.key(),
            swap_to_token_mint
        );

        // 分两段 swap：先把“奖励部分”换成目标币种（便于精确扣费），再把“本金部分”换成目标币种。
        // 注意：这里的 swap 输入来自 decrease_liquidity_v2 后的增量（user_token*_amount），不会动到用户原有余额。
        let (reward_other_in, principal_other_in, reward_target_direct) = if target_is_token0 {
            // token1 -> token0
            (
                reward_gross_1,
                user_token1_amount.checked_sub(reward_gross_1).unwrap_or(0),
                reward_gross_0,
            )
        } else {
            // token0 -> token1
            (
                reward_gross_0,
                user_token0_amount.checked_sub(reward_gross_0).unwrap_or(0),
                reward_gross_1,
            )
        };

        let input_is_token0 = !target_is_token0; // 目标是 token0 => 输入 token1；目标是 token1 => 输入 token0

        // 记录兑换前目标币种余额，用于计算 reward/principal 兑换的实际输出
        let target_balance_before_swap = if target_is_token0 {
            ctx.accounts.user_token0_account.amount
        } else {
            ctx.accounts.user_token1_account.amount
        };

        // 1) 先换 reward（如果有）
        let mut reward_out_in_target: u64 = 0;
        if reward_other_in > 0 {
            // 为了避免“用旧价格估 min_out”导致过严/过松，在 CPI swap_v2 前重新读取 pool_state 的最新价格。
            let sqrt_price_x64_for_min_out = {
                let pool_state = ctx.accounts.pool_state.load()?;
                pool_state.sqrt_price_x64
            };
            let swap_other_amount_threshold = utils::calc_min_amount_out(
                reward_other_in,
                input_is_token0,
                sqrt_price_x64_for_min_out,
                slippage_bps,
                ctx.accounts.amm_config.trade_fee_rate,
            )?;

            // dust 保护：如果预估的最小输出太小，则不执行 swap，直接把 reward_other_in 转给手续费地址
            if swap_other_amount_threshold == 0 {
                msg!(
                    "skip reward swap: amount_out_min {} =0, transfer input to fee",
                    swap_other_amount_threshold,
                );
                transfer_fee(
                    &ctx.accounts.user,
                    &ctx.accounts.fee_owner,
                    if input_is_token0 {
                        &ctx.accounts.user_token0_account
                    } else {
                        &ctx.accounts.user_token1_account
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
                    reward_other_in,
                )?;
            } else {
                swap_v2(
                    &ctx,
                    reward_other_in,
                    swap_other_amount_threshold,
                    0,
                    input_is_token0,
                    swap_remaining.to_vec(),
                )?;
                ctx.accounts.user_token0_account.reload()?;
                ctx.accounts.user_token1_account.reload()?;
                let target_balance_after_reward_swap = if target_is_token0 {
                    ctx.accounts.user_token0_account.amount
                } else {
                    ctx.accounts.user_token1_account.amount
                };
                reward_out_in_target = target_balance_after_reward_swap
                    .checked_sub(target_balance_before_swap)
                    .ok_or(LpDepositError::MathOverflow)?;
            }
        }

        // 2) reward 已全部在目标币种：计算并扣 fee（只对 reward 抽成）
        let reward_total_in_target = reward_target_direct
            .checked_add(reward_out_in_target)
            .ok_or(LpDepositError::MathOverflow)?;
        let integrator_fee_target = reward_total_in_target
            .checked_mul(fee_percent as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(LpDepositError::MathOverflow)?;

        transfer_fee(
            &ctx.accounts.user,
            &ctx.accounts.fee_owner,
            if target_is_token0 {
                &ctx.accounts.user_token0_account
            } else {
                &ctx.accounts.user_token1_account
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

        // 3) 再换本金（如果有）。为避免把已换出的 reward 再算一遍，这里只换剩余的 other token 增量。
        // 重新读取目标币种余额，用于计算本金兑换输出
        ctx.accounts.user_token0_account.reload()?;
        ctx.accounts.user_token1_account.reload()?;
        let target_balance_before_principal_swap = if target_is_token0 {
            ctx.accounts.user_token0_account.amount
        } else {
            ctx.accounts.user_token1_account.amount
        };
        let mut principal_out_in_target: u64 = 0;
        if principal_other_in > 0 {
            // 为了避免“用旧价格估 min_out”导致过严/过松，在 CPI swap_v2 前重新读取 pool_state 的最新价格。
            // 注：本次 swap 可能紧跟 reward swap 之后，因此必须再次读取。
            let sqrt_price_x64_for_min_out = {
                let pool_state = ctx.accounts.pool_state.load()?;
                pool_state.sqrt_price_x64
            };
            let swap_other_amount_threshold = utils::calc_min_amount_out(
                principal_other_in,
                input_is_token0,
                sqrt_price_x64_for_min_out,
                slippage_bps,
                ctx.accounts.amm_config.trade_fee_rate,
            )?;
            swap_v2(
                &ctx,
                principal_other_in,
                swap_other_amount_threshold,
                0,
                input_is_token0,
                swap_remaining.to_vec(),
            )?;
            ctx.accounts.user_token0_account.reload()?;
            ctx.accounts.user_token1_account.reload()?;
            let target_balance_after_principal_swap = if target_is_token0 {
                ctx.accounts.user_token0_account.amount
            } else {
                ctx.accounts.user_token1_account.amount
            };
            principal_out_in_target = target_balance_after_principal_swap
                .checked_sub(target_balance_before_principal_swap)
                .ok_or(LpDepositError::MathOverflow)?;
        }

        msg!(
            "swap_v2, swap_amount:{}, swap_other_amount_threshold: {}, is_token0: {}",
            reward_other_in + principal_other_in,
            0,
            input_is_token0
        );
        msg!(
            "balance,  user_token0_balance: {},  user_token1_balance: {}",
            user_token0_balance_after - user_token0_balance_before,
            user_token1_balance_after - user_token1_balance_before
        );

        // 事件按“兑换后”口径输出：只在目标币种上体现 principal/reward/fee，其它币种为 0
        if target_is_token0 {
            let principal_amount_0 = principal_expected_0
                .checked_add(principal_out_in_target)
                .ok_or(LpDepositError::MathOverflow)?;
            let reward_amount_0 = reward_total_in_target
                .checked_sub(integrator_fee_target)
                .ok_or(LpDepositError::MathOverflow)?;
            emit!(DecreaseLiquidityEvent {
                timestamp,
                user: ctx.accounts.user.key(),
                pool: ctx.accounts.pool_state.key(),
                token0_mint: ctx.accounts.vault_0_mint.key(),
                token1_mint: ctx.accounts.vault_1_mint.key(),
                principal_amount_0,
                principal_amount_1: 0,
                integrator_fee_0: integrator_fee_target,
                integrator_fee_1: 0,
                reward_amount_0,
                reward_amount_1: 0,
            });
        } else {
            let principal_amount_1 = principal_expected_1
                .checked_add(principal_out_in_target)
                .ok_or(LpDepositError::MathOverflow)?;
            let reward_amount_1 = reward_total_in_target
                .checked_sub(integrator_fee_target)
                .ok_or(LpDepositError::MathOverflow)?;
            emit!(DecreaseLiquidityEvent {
                timestamp,
                user: ctx.accounts.user.key(),
                pool: ctx.accounts.pool_state.key(),
                token0_mint: ctx.accounts.vault_0_mint.key(),
                token1_mint: ctx.accounts.vault_1_mint.key(),
                principal_amount_0: 0,
                principal_amount_1,
                integrator_fee_0: 0,
                integrator_fee_1: integrator_fee_target,
                reward_amount_0: 0,
                reward_amount_1,
            });
        }
        unwrap_wsol_ata_if_needed(
            &ctx.accounts.user,
            [
                &ctx.accounts.user_token0_account,
                &ctx.accounts.user_token1_account,
            ],
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            &ctx.accounts.associated_token_program,
        )?;
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
        // unwrap wSOL：关闭 authority 的 wSOL ATA，把 lamports 退回 authority
        unwrap_wsol_ata_if_needed(
            &ctx.accounts.user,
            [
                &ctx.accounts.user_token0_account,
                &ctx.accounts.user_token1_account,
            ],
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            &ctx.accounts.associated_token_program,
        )?;
        transfer_fee(
            &ctx.accounts.user,
            &ctx.accounts.fee_owner,
            &ctx.accounts.user_token0_account,
            &ctx.accounts.fee_token0_account,
            Some(&ctx.accounts.vault_0_mint),
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            &ctx.accounts.system_program,
            integrator_fee_0,
        )?;
        transfer_fee(
            &ctx.accounts.user,
            &ctx.accounts.fee_owner,
            &ctx.accounts.user_token1_account,
            &ctx.accounts.fee_token1_account,
            Some(&ctx.accounts.vault_1_mint),
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            &ctx.accounts.system_program,
            integrator_fee_1,
        )?;

        let principal_amount_0 = principal_expected_0;
        let principal_amount_1 = principal_expected_1;
        let reward_amount_0 = reward_gross_0
            .checked_sub(integrator_fee_0)
            .ok_or(LpDepositError::MathOverflow)?;
        let reward_amount_1 = reward_gross_1
            .checked_sub(integrator_fee_1)
            .ok_or(LpDepositError::MathOverflow)?;
        emit!(DecreaseLiquidityEvent {
            timestamp,
            user: ctx.accounts.user.key(),
            pool: ctx.accounts.pool_state.key(),
            token0_mint: ctx.accounts.vault_0_mint.key(),
            token1_mint: ctx.accounts.vault_1_mint.key(),
            principal_amount_0,
            principal_amount_1,
            integrator_fee_0,
            integrator_fee_1,
            reward_amount_0,
            reward_amount_1,
        });
    }

    Ok(())
}

fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
    swap_amount: u64,
    swap_other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_token0: bool,
    swap_remaining: Vec<AccountInfo<'info>>,
) -> Result<()> {
    // 使用解构简化代码
    let accounts = &ctx.accounts;
    let cpi_program = accounts.raydium_clmm_program.to_account_info();

    // 根据 is_token0 选择对应的账户（一次性解构）
    let (input_token, output_token, input_vault, output_vault, input_mint, output_mint) =
        match is_token0 {
            true => (
                &accounts.user_token0_account,
                &accounts.user_token1_account,
                &accounts.token_vault_0,
                &accounts.token_vault_1,
                &accounts.vault_0_mint,
                &accounts.vault_1_mint,
            ),
            false => (
                &accounts.user_token1_account,
                &accounts.user_token0_account,
                &accounts.token_vault_1,
                &accounts.token_vault_0,
                &accounts.vault_1_mint,
                &accounts.vault_0_mint,
            ),
        };

    let cpi_accounts = clmm_accounts::SwapSingleV2 {
        payer: accounts.user.to_account_info(),
        amm_config: accounts.amm_config.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        observation_state: accounts.observation_state.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        memo_program: accounts.memo_program.to_account_info(),
        input_token_account: input_token.to_account_info(),
        output_token_account: output_token.to_account_info(),
        input_vault: input_vault.to_account_info(),
        output_vault: output_vault.to_account_info(),
        input_vault_mint: input_mint.to_account_info(),
        output_vault_mint: output_mint.to_account_info(),
    };
    // 轻量校验 remaining_accounts：限制数量并要求 owner 为 Raydium CLMM program（tick array/bitmap 等应满足）
    require!(
        swap_remaining.len() <= 32,
        LpDepositError::InvalidRemainingAccounts
    );
    for acc in swap_remaining.iter() {
        require_keys_eq!(
            *acc.owner,
            accounts.raydium_clmm_program.key(),
            LpDepositError::InvalidRemainingAccounts
        );
    }

    let cpi_ctx =
        CpiContext::new(cpi_program.clone(), cpi_accounts).with_remaining_accounts(swap_remaining);
    clmm_cpi::swap_v2(
        cpi_ctx,
        swap_amount,
        swap_other_amount_threshold,
        sqrt_price_limit_x64,
        true,
    )?;

    Ok(())
}

pub fn transfer_fee<'info>(
    signer: &Signer<'info>,
    fee_owner: &SystemAccount<'info>,
    from: &InterfaceAccount<'info, TokenAccount>,
    to: &InterfaceAccount<'info, TokenAccount>,
    mint: Option<&InterfaceAccount<'info, Mint>>,
    token_program: &Program<'info, Token>,
    token_program_2022: Option<&Program<'info, Token2022>>,
    system_program: &Program<'info, System>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let mut token_program_info = token_program.to_account_info();

    // 如果手续费 mint 是 wSOL(native mint)，则直接转 SOL（lamports）给 sol_destination，而不是转 wSOL token
    // 注意：wSOL 的最小单位与 lamports 等价（9 decimals）
    if let Some(mint) = mint {
        if mint.key() == anchor_spl::token::spl_token::native_mint::ID {
            anchor_lang::system_program::transfer(
                CpiContext::new(
                    system_program.to_account_info(),
                    anchor_lang::system_program::Transfer {
                        from: signer.to_account_info(),
                        to: fee_owner.to_account_info(),
                    },
                ),
                amount,
            )?;
            return Ok(());
        }
    }

    match (mint, token_program_2022) {
        (Some(mint), Some(token_program_2022)) => {
            if from.to_account_info().owner == token_program_2022.key {
                token_program_info = token_program_2022.to_account_info()
            }
            token_2022::transfer_checked(
                CpiContext::new(
                    token_program_info,
                    token_2022::TransferChecked {
                        from: from.to_account_info(),
                        to: to.to_account_info(),
                        authority: signer.to_account_info(),
                        mint: mint.to_account_info(),
                    },
                ),
                amount,
                mint.decimals,
            )?;
        }
        _ => token::transfer(
            CpiContext::new(
                token_program_info,
                token::Transfer {
                    from: from.to_account_info(),
                    to: to.to_account_info(),
                    authority: signer.to_account_info(),
                },
            ),
            amount,
        )?,
    }

    Ok(())
}

// unwrap wSOL ATA：仅当传入的 token account 确实是 user 的 wSOL ATA 时才执行关闭（否则跳过）
fn unwrap_wsol_ata_if_needed<'info>(
    user: &Signer<'info>,
    token_accounts: [&InterfaceAccount<'info, TokenAccount>; 2],
    token_program: &Program<'info, Token>,
    token_program_2022: Option<&Program<'info, Token2022>>,
    associated_token_program: &Program<'info, AssociatedToken>,
) -> Result<()> {
    // wSOL = SPL Token native mint
    let wsol_mint_key = anchor_spl::token::spl_token::native_mint::ID;

    // native(wSOL) 账户允许在 amount != 0 时 close：lamports 会退回 destination（这里是 user），效果等同 unwrap
    for token_acc in token_accounts.iter() {
        // 仅关闭 user 的 ATA；不是就跳过（不报错）
        let token_program_for_ata = match token_program_2022 {
            Some(tp22) if token_acc.to_account_info().owner == tp22.key => tp22.key(),
            _ => token_program.key(),
        };
        let expected_ata = utils::derive_ata_address(
            &user.key(),
            &wsol_mint_key,
            &token_program_for_ata,
            &associated_token_program.key(),
        );
        if token_acc.key() != expected_ata {
            continue;
        }

        let token_account_info = token_acc.to_account_info();

        match token_program_2022 {
            Some(tp22) if token_account_info.owner == tp22.key => {
                token_2022::close_account(CpiContext::new(
                    tp22.to_account_info(),
                    token_2022::CloseAccount {
                        account: token_account_info,
                        destination: user.to_account_info(),
                        authority: user.to_account_info(),
                    },
                ))?;
            }
            _ => {
                token::close_account(CpiContext::new(
                    token_program.to_account_info(),
                    token::CloseAccount {
                        account: token_account_info,
                        destination: user.to_account_info(),
                        authority: user.to_account_info(),
                    },
                ))?;
            }
        }
    }

    Ok(())
}
