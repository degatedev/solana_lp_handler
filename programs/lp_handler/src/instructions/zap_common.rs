use anchor_lang::prelude::*;
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::libraries::{get_sqrt_price_at_tick, liquidity_math};
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{AmmConfig, ObservationState, PoolState};

use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

use crate::{utils, IncreaseLiquidityEvent, LpDepositError, SwapExecutedEvent};

/// 两个指令（`swap_and_deposit` / `increase_liquidity`）共享的账户访问接口。
///
/// 目的：把“最优 swap → swap_v2 → 计算 amount_0_max/amount_1_max/base_flag → 可选二次 swap → 事件”
/// 的重复逻辑抽到一个地方。
pub trait ZapCommonAccounts<'info> {
    fn raydium_clmm_program(&self) -> &Program<'info, AmmV3>;
    fn user(&self) -> &Signer<'info>;

    fn amm_config(&self) -> &Account<'info, AmmConfig>;
    fn pool_state(&self) -> &AccountLoader<'info, PoolState>;
    fn observation_state(&self) -> &AccountLoader<'info, ObservationState>;

    fn user_token0_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>>;
    fn user_token1_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>>;

    fn memo_program(&self) -> &Program<'info, Memo>;
    fn token_vault_0(&self) -> &Box<InterfaceAccount<'info, TokenAccount>>;
    fn token_vault_1(&self) -> &Box<InterfaceAccount<'info, TokenAccount>>;

    fn token_program(&self) -> &Program<'info, Token>;
    fn token_program_2022(&self) -> &Program<'info, Token2022>;

    fn vault_0_mint(&self) -> &Box<InterfaceAccount<'info, Mint>>;
    fn vault_1_mint(&self) -> &Box<InterfaceAccount<'info, Mint>>;
}

/// 为 Accounts struct 自动实现 `ZapCommonAccounts` 的宏。
///
/// 用法（在对应指令文件里）：
/// - `crate::impl_zap_common_accounts!(SwapAndDeposit<'info>);`
/// - `crate::impl_zap_common_accounts!(IncreaseLiquidity<'info>);`
#[macro_export]
macro_rules! impl_zap_common_accounts {
    ($ty:ty) => {
        impl<'info> $crate::instructions::zap_common::ZapCommonAccounts<'info> for $ty {
            fn raydium_clmm_program(&self) -> &Program<'info, AmmV3> {
                &self.raydium_clmm_program
            }
            fn user(&self) -> &Signer<'info> {
                &self.user
            }

            fn amm_config(&self) -> &Account<'info, AmmConfig> {
                self.amm_config.as_ref()
            }
            fn pool_state(&self) -> &AccountLoader<'info, PoolState> {
                &self.pool_state
            }
            fn observation_state(&self) -> &AccountLoader<'info, ObservationState> {
                &self.observation_state
            }

            fn user_token0_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>> {
                &mut self.user_token0_account
            }
            fn user_token1_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>> {
                &mut self.user_token1_account
            }

            fn memo_program(&self) -> &Program<'info, Memo> {
                &self.memo_program
            }
            fn token_vault_0(&self) -> &Box<InterfaceAccount<'info, TokenAccount>> {
                &self.token_vault_0
            }
            fn token_vault_1(&self) -> &Box<InterfaceAccount<'info, TokenAccount>> {
                &self.token_vault_1
            }

            fn token_program(&self) -> &Program<'info, Token> {
                &self.token_program
            }
            fn token_program_2022(&self) -> &Program<'info, Token2022> {
                &self.token_program_2022
            }

            fn vault_0_mint(&self) -> &Box<InterfaceAccount<'info, Mint>> {
                &self.vault_0_mint
            }
            fn vault_1_mint(&self) -> &Box<InterfaceAccount<'info, Mint>> {
                &self.vault_1_mint
            }
        }
    };
}

pub struct ZapPlan<'info> {
    pub is_token0: bool,
    pub tick_spacing: u16,

    pub balance_0_before: u64,
    pub balance_1_before: u64,

    pub swap_remaining: Vec<AccountInfo<'info>>,
    pub action_remaining: Vec<AccountInfo<'info>>,

    pub amount_0_max: u64,
    pub amount_1_max: u64,
    pub base_flag: Option<bool>,

    /// 给下游 CPI 使用的 liquidity（由 Raydium 的数学库根据 amount_0_max/amount_1_max 计算得到）
    pub computed_liquidity: u128,
}

pub fn prepare_zap_plan_and_swap_if_needed<'info>(
    accounts: &mut dyn ZapCommonAccounts<'info>,
    remaining_accounts: &[AccountInfo<'info>],
    deposit_amount: u64,
    deposit_mint: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity_param: u128,
    slippage_bps: u16,
    lp_slippage_bps: u16,
) -> Result<ZapPlan<'info>> {
    require!(
        tick_lower_index < tick_upper_index,
        LpDepositError::InvalidTickRange
    );
    require!(
        deposit_mint == accounts.vault_0_mint().key()
            || deposit_mint == accounts.vault_1_mint().key(),
        LpDepositError::InvalidDepositMint
    );
    require!(slippage_bps < 5_000, LpDepositError::InvalidSlippage);

    let is_token0 = deposit_mint == accounts.vault_0_mint().key();

    let (swap_amount_in, _swap_amount_out, tick_spacing) = {
        let pool_state = accounts.pool_state().load()?;
        let current_tick = pool_state.tick_current;
        let tick_spacing = pool_state.tick_spacing;
        let (swap_amount, swap_amount_out) = utils::calculate_optimal_swap_amount(
            deposit_amount,
            is_token0,
            current_tick,
            tick_lower_index,
            tick_upper_index,
            pool_state.sqrt_price_x64,
            liquidity_param,
        )?;
        (swap_amount, swap_amount_out, tick_spacing)
    };

    let balance_0_before = accounts.user_token0_account().amount;
    let balance_1_before = accounts.user_token1_account().amount;

    // remaining_accounts：用 programId 作为分隔符拆为两段（与现有逻辑一致）
    let sep = crate::ID;
    let sep_index = remaining_accounts
        .iter()
        .position(|a| a.key() == sep)
        .ok_or(LpDepositError::InvalidRemainingAccounts)?;
    let (swap_remaining_slice, rest) = remaining_accounts.split_at(sep_index);
    let action_remaining_slice = &rest[1..]; // 跳过分隔符本身

    let swap_remaining: Vec<AccountInfo<'info>> = swap_remaining_slice.to_vec();
    let action_remaining: Vec<AccountInfo<'info>> = action_remaining_slice.to_vec();

    let mut swap_amount_min = 0u64;
    if swap_amount_in > 0 {
        // 用最新 sqrt_price 估算 min_out（避免价格变动导致 min_out 偏差）
        let sqrt_price_x64_for_min_out = {
            let pool_state = accounts.pool_state().load()?;
            pool_state.sqrt_price_x64
        };

        let mut min_amount_out = utils::calc_min_amount_out(
            swap_amount_in,
            is_token0,
            sqrt_price_x64_for_min_out,
            slippage_bps,
            accounts.amm_config().trade_fee_rate,
        )?;
        swap_amount_min = swap_amount_in;
        if swap_amount_in != deposit_amount {
            swap_amount_min = utils::apply_slippage_bps_floor(swap_amount_in, lp_slippage_bps)?;
            min_amount_out = utils::calc_min_amount_out(
                swap_amount_min,
                is_token0,
                sqrt_price_x64_for_min_out,
                slippage_bps,
                accounts.amm_config().trade_fee_rate,
            )?;
        }

        swap_v2_common(
            accounts,
            swap_amount_min,
            min_amount_out,
            0,
            is_token0,
            swap_remaining.clone(),
        )?;

        accounts.user_token0_account().reload()?;
        accounts.user_token1_account().reload()?;

        let amount_out_after = if is_token0 {
            accounts
                .user_token1_account()
                .amount
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            accounts
                .user_token0_account()
                .amount
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?
        };

        emit!(SwapExecutedEvent {
            user: accounts.user().key(),
            pool: accounts.pool_state().key(),
            amount_in: swap_amount_min,
            amount_out: amount_out_after,
            amount_out_min: min_amount_out,
            is_token0_input: is_token0,
            token0_mint: accounts.vault_0_mint().key(),
            token1_mint: accounts.vault_1_mint().key(),
            slippage_bps,
        });
    }

    let balance_0_after = accounts.user_token0_account().amount;
    let balance_1_after = accounts.user_token1_account().amount;

    let (amount_0_max, amount_1_max) = if is_token0 {
        (
            deposit_amount
                .checked_sub(swap_amount_min)
                .ok_or(LpDepositError::MathOverflow)?,
            balance_1_after
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?,
        )
    } else {
        (
            balance_0_after
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?,
            deposit_amount
                .checked_sub(swap_amount_min)
                .ok_or(LpDepositError::MathOverflow)?,
        )
    };

    let base_flag = if amount_0_max == 0 {
        Some(false)
    } else if amount_1_max == 0 {
        Some(true)
    } else {
        Some(true)
    };

    // 计算给下游 CPI 使用的 liquidity（按 swap 后的最新池价）
    let sqrt_price_x64 = {
        let pool_state = accounts.pool_state().load()?;
        pool_state.sqrt_price_x64
    };
    let sqrt_ratio_a_x64 = get_sqrt_price_at_tick(tick_lower_index)?;
    let sqrt_ratio_b_x64 = get_sqrt_price_at_tick(tick_upper_index)?;
    let computed_liquidity = liquidity_math::get_liquidity_from_amounts(
        sqrt_price_x64,
        sqrt_ratio_a_x64,
        sqrt_ratio_b_x64,
        amount_0_max,
        amount_1_max,
    );

    Ok(ZapPlan {
        is_token0,
        tick_spacing,
        balance_0_before,
        balance_1_before,
        swap_remaining,
        action_remaining,
        amount_0_max,
        amount_1_max,
        base_flag,
        computed_liquidity,
    })
}

pub fn swap_back_remaining_and_emit_increase_event<'info>(
    accounts: &mut dyn ZapCommonAccounts<'info>,
    deposit_amount: u64,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity_param: u128,
    slippage_bps: u16,
    is_token0: bool,
    balance_0_before: u64,
    balance_1_before: u64,
    amount_0_max: u64,
    amount_1_max: u64,
    swap_remaining: Vec<AccountInfo<'info>>,
    position_nft_mint: Option<Pubkey>,
) -> Result<u64> {
    accounts.user_token0_account().reload()?;
    accounts.user_token1_account().reload()?;

    let amount_0_after = accounts.user_token0_account().amount;
    let amount_1_after = accounts.user_token1_account().amount;

    // 剩余资产：与原实现保持一致（相对 swap 前余额）
    let remaining_amount = if is_token0 {
        amount_1_after.checked_sub(balance_1_before).unwrap_or(0)
    } else {
        amount_0_after.checked_sub(balance_0_before).unwrap_or(0)
    };

    if remaining_amount > 0 {
        let sqrt_price_x64 = {
            let pool_state = accounts.pool_state().load()?;
            pool_state.sqrt_price_x64
        };
        let min_amount_out = utils::calc_min_amount_out(
            remaining_amount,
            !is_token0,
            sqrt_price_x64,
            slippage_bps,
            accounts.amm_config().trade_fee_rate,
        )?;

        swap_v2_common(
            accounts,
            remaining_amount,
            min_amount_out,
            0,
            !is_token0,
            swap_remaining,
        )?;
    }

    accounts.user_token0_account().reload()?;
    accounts.user_token1_account().reload()?;

    let amount_0_after = accounts.user_token0_account().amount;
    let amount_1_after = accounts.user_token1_account().amount;

    let return_amount = if is_token0 {
        let amount_0_spent = balance_0_before
            .checked_sub(amount_0_after)
            .ok_or(LpDepositError::MathOverflow)?;
        deposit_amount
            .checked_sub(amount_0_spent)
            .ok_or(LpDepositError::MathOverflow)?
    } else {
        let amount_1_spent = balance_1_before
            .checked_sub(amount_1_after)
            .ok_or(LpDepositError::MathOverflow)?;
        deposit_amount
            .checked_sub(amount_1_spent)
            .ok_or(LpDepositError::MathOverflow)?
    };

    emit!(IncreaseLiquidityEvent {
        user: accounts.user().key(),
        pool: accounts.pool_state().key(),
        position_nft_mint,
        amount_0: amount_0_max,
        amount_1: amount_1_max,
        deposit_amount,
        return_amount,
        token0_mint: accounts.vault_0_mint().key(),
        token1_mint: accounts.vault_1_mint().key(),
        tick_lower_index,
        tick_upper_index,
        liquidity: liquidity_param,
    });

    Ok(return_amount)
}

/// 公开的 swap_v2 工具：仅依赖 AccountInfo，不依赖 `ZapCommonAccounts`。
///
/// 适用于 `decrease_liquidity` 这类指令：它们可能没有/不想实现 `ZapCommonAccounts`，
/// 但 swap_v2 的账户结构仍然是固定的。
pub fn swap_v2_accounts<'info>(
    raydium_clmm_program: AccountInfo<'info>,
    payer: AccountInfo<'info>,
    amm_config: AccountInfo<'info>,
    pool_state: AccountInfo<'info>,
    observation_state: AccountInfo<'info>,
    token_program: AccountInfo<'info>,
    token_program_2022: AccountInfo<'info>,
    memo_program: AccountInfo<'info>,
    input_token_account: AccountInfo<'info>,
    output_token_account: AccountInfo<'info>,
    input_vault: AccountInfo<'info>,
    output_vault: AccountInfo<'info>,
    input_vault_mint: AccountInfo<'info>,
    output_vault_mint: AccountInfo<'info>,
    swap_remaining: Vec<AccountInfo<'info>>,
    swap_amount: u64,
    swap_other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
) -> Result<()> {
    require!(
        swap_remaining.len() <= 32,
        LpDepositError::InvalidRemainingAccounts
    );
    let raydium_program_key = raydium_clmm_program.key();
    for acc in swap_remaining.iter() {
        require_keys_eq!(
            *acc.owner,
            raydium_program_key,
            LpDepositError::InvalidRemainingAccounts
        );
    }

    let cpi_accounts = clmm_accounts::SwapSingleV2 {
        payer,
        amm_config,
        pool_state,
        observation_state,
        token_program,
        token_program_2022,
        memo_program,
        input_token_account,
        output_token_account,
        input_vault,
        output_vault,
        input_vault_mint,
        output_vault_mint,
    };

    let cpi_ctx =
        CpiContext::new(raydium_clmm_program, cpi_accounts).with_remaining_accounts(swap_remaining);
    clmm_cpi::swap_v2(
        cpi_ctx,
        swap_amount,
        swap_other_amount_threshold,
        sqrt_price_limit_x64,
        true,
    )?;

    Ok(())
}

fn swap_v2_common<'info>(
    accounts: &mut dyn ZapCommonAccounts<'info>,
    swap_amount: u64,
    swap_other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_token0: bool,
    swap_remaining: Vec<AccountInfo<'info>>,
) -> Result<()> {
    let cpi_program = accounts.raydium_clmm_program().to_account_info();

    // 先把会用到的 AccountInfo “拷贝出来”，避免 &mut self 多重借用冲突
    let payer_ai = accounts.user().to_account_info();
    let amm_config_ai = accounts.amm_config().to_account_info();
    let pool_state_ai = accounts.pool_state().to_account_info();
    let observation_state_ai = accounts.observation_state().to_account_info();
    let token_program_ai = accounts.token_program().to_account_info();
    let token_program_2022_ai = accounts.token_program_2022().to_account_info();
    let memo_program_ai = accounts.memo_program().to_account_info();

    let user_token0_ai = accounts.user_token0_account().to_account_info();
    let user_token1_ai = accounts.user_token1_account().to_account_info();
    let token_vault_0_ai = accounts.token_vault_0().to_account_info();
    let token_vault_1_ai = accounts.token_vault_1().to_account_info();
    let vault_0_mint_ai = accounts.vault_0_mint().to_account_info();
    let vault_1_mint_ai = accounts.vault_1_mint().to_account_info();

    let (
        input_token_ai,
        output_token_ai,
        input_vault_ai,
        output_vault_ai,
        input_mint_ai,
        output_mint_ai,
    ) = if is_token0 {
        (
            user_token0_ai,
            user_token1_ai,
            token_vault_0_ai,
            token_vault_1_ai,
            vault_0_mint_ai,
            vault_1_mint_ai,
        )
    } else {
        (
            user_token1_ai,
            user_token0_ai,
            token_vault_1_ai,
            token_vault_0_ai,
            vault_1_mint_ai,
            vault_0_mint_ai,
        )
    };

    swap_v2_accounts(
        cpi_program,
        payer_ai,
        amm_config_ai,
        pool_state_ai,
        observation_state_ai,
        token_program_ai,
        token_program_2022_ai,
        memo_program_ai,
        input_token_ai,
        output_token_ai,
        input_vault_ai,
        output_vault_ai,
        input_mint_ai,
        output_mint_ai,
        swap_remaining,
        swap_amount,
        swap_other_amount_threshold,
        sqrt_price_limit_x64,
    )
}
