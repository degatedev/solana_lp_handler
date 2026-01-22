use anchor_lang::prelude::*;
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::libraries::{get_sqrt_price_at_tick, liquidity_math, U256};
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
    pub tick_spacing: u16,

    pub balance_0_before: u64,
    pub balance_1_before: u64,

    /// 执行“主配平 swap”后的余额（也就是进入 open_position/increase_liquidity CPI 之前的余额）
    pub balance_0_pre_cpi: u64,
    pub balance_1_pre_cpi: u64,

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
    amount_0_in: u64,
    amount_1_in: u64,
    return_mint: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
    slippage_bps: u16,
    lp_slippage_bps: u16,
) -> Result<ZapPlan<'info>> {
    require!(
        tick_lower_index < tick_upper_index,
        LpDepositError::InvalidTickRange
    );
    require!(
        amount_0_in > 0 || amount_1_in > 0,
        LpDepositError::InvalidDepositAmount
    );
    // return_mint 必须是池子的 token0 或 token1
    require!(
        return_mint == accounts.vault_0_mint().key()
            || return_mint == accounts.vault_1_mint().key(),
        LpDepositError::InvalidDepositMint
    );
    require!(slippage_bps < 5_000, LpDepositError::InvalidSlippage);

    let tick_spacing = {
        let pool_state = accounts.pool_state().load()?;
        pool_state.tick_spacing
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

    // ====== 计算是否需要主配平 swap（最多一次）======
    // 说明：此处仅用当前价格做近似配平，真实价格冲击由 Raydium 的 slippage/min_out 兜底。
    let trade_fee_rate = accounts.amm_config().trade_fee_rate;
    let sqrt_price_x64 = {
        let pool_state = accounts.pool_state().load()?;
        pool_state.sqrt_price_x64
    };
    let sa = get_sqrt_price_at_tick(tick_lower_index)?;
    let sb = get_sqrt_price_at_tick(tick_upper_index)?;
    let sp = sqrt_price_x64;

    let (mut swap_in, mut swap_input_is_token0) = (0u64, true);
    if sp <= sa {
        // 区间在现价上方：只需 token0
        if amount_1_in > 0 {
            swap_in = amount_1_in;
            swap_input_is_token0 = false; // token1 -> token0
        }
    } else if sp >= sb {
        // 区间在现价下方：只需 token1
        if amount_0_in > 0 {
            swap_in = amount_0_in;
            swap_input_is_token0 = true; // token0 -> token1
        }
    } else {
        // 区间跨现价：按目标配比 R* = token1/token0 配平
        let (r_num, r_den) = compute_rstar_ratio(sa, sp, sb)?;
        // 当前配比（token1/token0）
        let cur_left = U256::from(amount_1_in as u128) * r_den;
        let cur_right = U256::from(amount_0_in as u128) * r_num;
        if cur_left > cur_right {
            // token1 偏多：swap token1 -> token0
            swap_input_is_token0 = false;
            swap_in = solve_swap_amount_for_target_ratio(
                amount_0_in,
                amount_1_in,
                false,
                r_num,
                r_den,
                sp,
                trade_fee_rate,
            )?;
        } else if cur_left < cur_right {
            // token0 偏多：swap token0 -> token1
            swap_input_is_token0 = true;
            swap_in = solve_swap_amount_for_target_ratio(
                amount_0_in,
                amount_1_in,
                true,
                r_num,
                r_den,
                sp,
                trade_fee_rate,
            )?;
        }
    }

    // 执行主配平 swap（如需要）
    let mut amount_0_max = amount_0_in;
    let mut amount_1_max = amount_1_in;
    let balance_0_pre_cpi: u64;
    let balance_1_pre_cpi: u64;

    if swap_in > 0 {
        // lp_slippage_bps 只影响 swap_in 的“实际执行量”，保持与旧逻辑一致（向下取整）
        let swap_amount_min = utils::apply_slippage_bps_floor(swap_in, lp_slippage_bps)?;

        // 用最新 sqrt_price 估算 min_out（避免价格变动导致 min_out 偏差）
        let sqrt_price_x64_for_min_out = {
            let pool_state = accounts.pool_state().load()?;
            pool_state.sqrt_price_x64
        };
        let min_amount_out = utils::calc_min_amount_out(
            swap_amount_min,
            swap_input_is_token0,
            sqrt_price_x64_for_min_out,
            slippage_bps,
            trade_fee_rate,
        )?;

        swap_v2_common(
            accounts,
            swap_amount_min,
            min_amount_out,
            0,
            swap_input_is_token0,
            swap_remaining.clone(),
        )?;

        accounts.user_token0_account().reload()?;
        accounts.user_token1_account().reload()?;

        let balance_0_after_swap = accounts.user_token0_account().amount;
        let balance_1_after_swap = accounts.user_token1_account().amount;

        // 输出币种的实际增量
        let amount_out_after = if swap_input_is_token0 {
            balance_1_after_swap
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            balance_0_after_swap
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?
        };

        emit!(SwapExecutedEvent {
            user: accounts.user().key(),
            pool: accounts.pool_state().key(),
            amount_in: swap_amount_min,
            amount_out: amount_out_after,
            amount_out_min: min_amount_out,
            is_token0_input: swap_input_is_token0,
            token0_mint: accounts.vault_0_mint().key(),
            token1_mint: accounts.vault_1_mint().key(),
            slippage_bps,
        });

        // 计算 CPI 可用的 max（= 输入预算经过 swap 后的可用额度）
        if swap_input_is_token0 {
            // token0 -> token1：token0 减少 swap_in，token1 增加 out
            amount_0_max = amount_0_in
                .checked_sub(swap_amount_min)
                .ok_or(LpDepositError::MathOverflow)?;
            amount_1_max = amount_1_in
                .checked_add(amount_out_after)
                .ok_or(LpDepositError::MathOverflow)?;
        } else {
            // token1 -> token0：token1 减少 swap_in，token0 增加 out
            amount_1_max = amount_1_in
                .checked_sub(swap_amount_min)
                .ok_or(LpDepositError::MathOverflow)?;
            amount_0_max = amount_0_in
                .checked_add(amount_out_after)
                .ok_or(LpDepositError::MathOverflow)?;
        }

        balance_0_pre_cpi = balance_0_after_swap;
        balance_1_pre_cpi = balance_1_after_swap;
    } else {
        // 没有 swap：进入 CPI 前余额就是初始余额
        balance_0_pre_cpi = balance_0_before;
        balance_1_pre_cpi = balance_1_before;
    }

    let base_flag = if amount_0_max == 0 {
        Some(false)
    } else if amount_1_max == 0 {
        Some(true)
    } else {
        Some(true)
    };

    // 计算给下游 CPI 使用的 liquidity（按最新池价）
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
        tick_spacing,
        balance_0_before,
        balance_1_before,
        balance_0_pre_cpi,
        balance_1_pre_cpi,
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
    amount_0_in: u64,
    amount_1_in: u64,
    return_mint: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
    computed_liquidity: u128,
    slippage_bps: u16,
    balance_0_pre_cpi: u64,
    balance_1_pre_cpi: u64,
    amount_0_max: u64,
    amount_1_max: u64,
    swap_remaining: Vec<AccountInfo<'info>>,
    position_nft_mint: Option<Pubkey>,
) -> Result<u64> {
    accounts.user_token0_account().reload()?;
    accounts.user_token1_account().reload()?;

    let balance_0_after_cpi = accounts.user_token0_account().amount;
    let balance_1_after_cpi = accounts.user_token1_account().amount;

    // CPI 实际花费（<= amount_max）
    let spent_0 = balance_0_pre_cpi
        .checked_sub(balance_0_after_cpi)
        .unwrap_or(0);
    let spent_1 = balance_1_pre_cpi
        .checked_sub(balance_1_after_cpi)
        .unwrap_or(0);

    let leftover_0 = amount_0_max.checked_sub(spent_0).unwrap_or(0);
    let leftover_1 = amount_1_max.checked_sub(spent_1).unwrap_or(0);

    // 把非 return_mint 的剩余统一兑换成 return_mint
    let return_amount: u64;
    let vault0 = accounts.vault_0_mint().key();
    let vault1 = accounts.vault_1_mint().key();
    require!(
        return_mint == vault0 || return_mint == vault1,
        LpDepositError::InvalidDepositMint
    );

    if return_mint == vault0 {
        // token0 作为返回币种
        let mut out_from_swap = 0u64;
        if leftover_1 > 0 {
            let before0 = accounts.user_token0_account().amount;
            let sqrt_price_x64 = {
                let pool_state = accounts.pool_state().load()?;
                pool_state.sqrt_price_x64
            };
            let min_out = utils::calc_min_amount_out(
                leftover_1,
                false,
                sqrt_price_x64,
                slippage_bps,
                accounts.amm_config().trade_fee_rate,
            )?;
            swap_v2_common(
                accounts,
                leftover_1,
                min_out,
                0,
                false,
                swap_remaining.clone(),
            )?;
            accounts.user_token0_account().reload()?;
            out_from_swap = accounts
                .user_token0_account()
                .amount
                .checked_sub(before0)
                .unwrap_or(0);
        }
        return_amount = leftover_0.checked_add(out_from_swap).unwrap_or(0);
    } else {
        // token1 作为返回币种
        let mut out_from_swap = 0u64;
        if leftover_0 > 0 {
            let before1 = accounts.user_token1_account().amount;
            let sqrt_price_x64 = {
                let pool_state = accounts.pool_state().load()?;
                pool_state.sqrt_price_x64
            };
            let min_out = utils::calc_min_amount_out(
                leftover_0,
                true,
                sqrt_price_x64,
                slippage_bps,
                accounts.amm_config().trade_fee_rate,
            )?;
            swap_v2_common(
                accounts,
                leftover_0,
                min_out,
                0,
                true,
                swap_remaining.clone(),
            )?;
            accounts.user_token1_account().reload()?;
            out_from_swap = accounts
                .user_token1_account()
                .amount
                .checked_sub(before1)
                .unwrap_or(0);
        }
        return_amount = leftover_1.checked_add(out_from_swap).unwrap_or(0);
    }

    emit!(IncreaseLiquidityEvent {
        user: accounts.user().key(),
        pool: accounts.pool_state().key(),
        position_nft_mint,
        amount_0: spent_0,
        amount_1: spent_1,
        token0_mint: vault0,
        token1_mint: vault1,
        tick_lower_index,
        tick_upper_index,
        liquidity: computed_liquidity,
        amount_0_in,
        amount_1_in,
        return_mint,
        return_amount,
    });

    Ok(return_amount)
}

fn compute_rstar_ratio(sa: u128, sp: u128, sb: u128) -> Result<(U256, U256)> {
    // R* = (sp - sa) * (sp * sb) / (sb - sp)
    require!(sb > sp, LpDepositError::InvalidSqrtPrice);
    require!(sp > sa, LpDepositError::InvalidSqrtPrice);

    let sp_u = U256::from(sp);
    let sa_u = U256::from(sa);
    let sb_u = U256::from(sb);

    let num = (sp_u - sa_u) * (sp_u * sb_u);
    let den = sb_u - sp_u;
    Ok((num, den))
}

fn quote_out_no_slippage(
    amount_in: u64,
    input_is_token0: bool,
    sqrt_price_x64: u128,
    trade_fee_rate: u32,
) -> Result<u64> {
    require!(trade_fee_rate <= 1_000_000, LpDepositError::MathOverflow);

    let amount_in_u = U256::from(amount_in as u128);
    let sqrt_price = U256::from(sqrt_price_x64);
    let price_q128 = (sqrt_price * sqrt_price) >> 64;
    require!(!price_q128.is_zero(), LpDepositError::InvalidSqrtPrice);

    let fee_factor = U256::from(1_000_000u128 - trade_fee_rate as u128);
    let out_before_fee = if input_is_token0 {
        let raw = (amount_in_u * price_q128) >> 64;
        raw
    } else {
        let inv_price = ((U256::one() << 64) << 64) / price_q128;
        let raw = (amount_in_u * inv_price) >> 64;
        raw
    };

    let out_after_fee = (out_before_fee * fee_factor) / U256::from(1_000_000u128);
    require!(
        out_after_fee <= U256::from(u64::MAX as u128),
        LpDepositError::MathOverflow
    );
    Ok(out_after_fee.as_u64())
}

fn solve_swap_amount_for_target_ratio(
    amount_0_in: u64,
    amount_1_in: u64,
    input_is_token0: bool,
    r_num: U256,
    r_den: U256,
    sqrt_price_x64: u128,
    trade_fee_rate: u32,
) -> Result<u64> {
    // 二分搜索 swap_in，使得 swap 后 (token1/token0) 尽量接近 R*
    let mut lo: u64 = 0;
    let mut hi: u64 = if input_is_token0 {
        amount_0_in
    } else {
        amount_1_in
    };

    // 限制迭代，避免计算量过大
    for _ in 0..28 {
        if lo >= hi {
            break;
        }
        let mid = (lo + hi) / 2;
        if mid == lo {
            break;
        }
        let out = quote_out_no_slippage(mid, input_is_token0, sqrt_price_x64, trade_fee_rate)?;

        let (new0, new1) = if input_is_token0 {
            (
                (amount_0_in as i128 - mid as i128).max(0) as u64,
                amount_1_in
                    .checked_add(out)
                    .ok_or(LpDepositError::MathOverflow)?,
            )
        } else {
            (
                amount_0_in
                    .checked_add(out)
                    .ok_or(LpDepositError::MathOverflow)?,
                (amount_1_in as i128 - mid as i128).max(0) as u64,
            )
        };

        // 比较 new1/new0 与 R*：new1 * r_den ? new0 * r_num
        let left = U256::from(new1 as u128) * r_den;
        let right = U256::from(new0 as u128) * r_num;

        if input_is_token0 {
            // ratio 随 mid 增大而增大
            if left < right {
                lo = mid;
            } else {
                hi = mid;
            }
        } else {
            // token1->token0 时 ratio 随 mid 增大而减小
            if left > right {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }

    Ok(hi)
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
