use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token;
use anchor_spl::token::Token;
use anchor_spl::token_2022::{self, Token2022};
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::libraries::{get_sqrt_price_at_tick, liquidity_math, U256};
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{AmmConfig, ObservationState, PoolState};

use crate::{utils, LpDepositError, LpHandlerIncreaseLiquidityEvent};

/// 两个指令（`swap_and_deposit` / `increase_liquidity`）共享的账户访问接口。
///
/// 目的：把“最优 swap → swap_v2 → 计算 amount_0_max/amount_1_max/base_flag → 可选二次 swap → 事件”
/// 的重复逻辑抽到一个地方。
pub trait ZapCommonAccounts<'info> {
    fn raydium_clmm_program(&self) -> &Program<'info, AmmV3>;
    fn signer(&self) -> &Signer<'info>;

    fn fee_owner(&self) -> &SystemAccount<'info>;
    fn fee_token0_account(&self) -> &Box<InterfaceAccount<'info, TokenAccount>>;
    fn fee_token1_account(&self) -> &Box<InterfaceAccount<'info, TokenAccount>>;

    fn amm_config(&self) -> &Account<'info, AmmConfig>;
    fn pool_state(&self) -> &AccountLoader<'info, PoolState>;
    fn observation_state(&self) -> &AccountLoader<'info, ObservationState>;

    fn signer_token0_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>>;
    fn signer_token1_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>>;

    fn memo_program(&self) -> &Program<'info, Memo>;
    fn token_vault_0(&self) -> &Box<InterfaceAccount<'info, TokenAccount>>;
    fn token_vault_1(&self) -> &Box<InterfaceAccount<'info, TokenAccount>>;

    fn token_program(&self) -> &Program<'info, Token>;
    fn token_program_2022(&self) -> &Program<'info, Token2022>;
    fn associated_token_program(&self) -> &Program<'info, AssociatedToken>;

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
            fn signer(&self) -> &Signer<'info> {
                &self.signer
            }

            fn fee_owner(&self) -> &SystemAccount<'info> {
                &self.fee_owner
            }
            fn fee_token0_account(&self) -> &Box<InterfaceAccount<'info, TokenAccount>> {
                &self.fee_token0_account
            }
            fn fee_token1_account(&self) -> &Box<InterfaceAccount<'info, TokenAccount>> {
                &self.fee_token1_account
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

            fn signer_token0_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>> {
                &mut self.signer_token0_account
            }
            fn signer_token1_account(&mut self) -> &mut Box<InterfaceAccount<'info, TokenAccount>> {
                &mut self.signer_token1_account
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
            fn associated_token_program(&self) -> &Program<'info, AssociatedToken> {
                &self.associated_token_program
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

    /// swap_v2 所需 remaining accounts（来自 ctx.remaining_accounts 的 slice，不在这里分配 Vec）
    pub swap_remaining: &'info [AccountInfo<'info>],
    /// open_position/increase_liquidity 所需 remaining accounts（来自 ctx.remaining_accounts 的 slice）
    pub action_remaining: &'info [AccountInfo<'info>],

    pub amount_0_max: u64,
    pub amount_1_max: u64,
    pub base_flag: Option<bool>,

    /// 给下游 CPI 使用的 liquidity（由 Raydium 的数学库根据 amount_0_max/amount_1_max 计算得到）
    pub computed_liquidity: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum QuotedZapMode {
    InRange = 0,
    OutOfRangeToken0Only = 1,
    OutOfRangeToken1Only = 2,
}

impl QuotedZapMode {
    fn try_from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::InRange),
            1 => Ok(Self::OutOfRangeToken0Only),
            2 => Ok(Self::OutOfRangeToken1Only),
            _ => err!(LpDepositError::InvalidQuotedMode),
        }
    }
}

fn determine_zap_mode(
    tick_lower_index: i32,
    tick_upper_index: i32,
    current_sqrt_price_x64: u128,
) -> Result<QuotedZapMode> {
    let sa = get_sqrt_price_at_tick(tick_lower_index)?;
    let sb = get_sqrt_price_at_tick(tick_upper_index)?;

    if current_sqrt_price_x64 <= sa {
        Ok(QuotedZapMode::OutOfRangeToken0Only)
    } else if current_sqrt_price_x64 >= sb {
        Ok(QuotedZapMode::OutOfRangeToken1Only)
    } else {
        Ok(QuotedZapMode::InRange)
    }
}

fn validate_quoted_mode_and_price(
    tick_lower_index: i32,
    tick_upper_index: i32,
    current_sqrt_price_x64: u128,
    quoted_mode: u8,
    quoted_sqrt_price_x64: u128,
    slippage_bps: u16,
) -> Result<QuotedZapMode> {
    validate_price_floor_from_quote(current_sqrt_price_x64, quoted_sqrt_price_x64, slippage_bps)?;

    let quoted_mode = QuotedZapMode::try_from_u8(quoted_mode)?;
    let actual_mode = determine_zap_mode(
        tick_lower_index,
        tick_upper_index,
        current_sqrt_price_x64,
    )?;
    require!(actual_mode == quoted_mode, LpDepositError::QuotedModeMismatch);
    Ok(actual_mode)
}

pub(crate) fn validate_price_floor_from_quote(
    current_sqrt_price_x64: u128,
    quoted_sqrt_price_x64: u128,
    slippage_bps: u16,
) -> Result<()> {
    require!(slippage_bps <= 10_000, LpDepositError::InvalidSlippage);

    let current_price_q64 = (U256::from(current_sqrt_price_x64) * U256::from(current_sqrt_price_x64)) >> 64;
    let quoted_price_q64 = (U256::from(quoted_sqrt_price_x64) * U256::from(quoted_sqrt_price_x64)) >> 64;
    let lhs = current_price_q64 * U256::from(10_000u128);
    let rhs = quoted_price_q64 * U256::from(10_000u128 - slippage_bps as u128);
    require!(lhs >= rhs, LpDepositError::QuotedPriceBelowMinimum);
    Ok(())
}

pub fn prepare_zap_plan_and_swap_if_needed<'info>(
    accounts: &mut dyn ZapCommonAccounts<'info>,
    remaining_accounts: &'info [AccountInfo<'info>],
    amount_0_in: u64,
    amount_1_in: u64,
    return_mint: Option<Pubkey>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    quoted_mode: u8,
    quoted_sqrt_price_x64: u128,
    slippage_bps: u16,
    swap_amount_in: u64,
    swap_min_out: u64,
    swap_input_is_token0: bool,
) -> Result<ZapPlan<'info>> {
    require!(
        tick_lower_index < tick_upper_index,
        LpDepositError::InvalidTickRange
    );
    require!(
        amount_0_in > 0 || amount_1_in > 0,
        LpDepositError::InvalidDepositAmount
    );
    // return_mint 若提供，必须是池子的 token0 或 token1
    if let Some(return_mint) = return_mint {
        require!(
            return_mint == accounts.vault_0_mint().key()
                || return_mint == accounts.vault_1_mint().key(),
            LpDepositError::InvalidDepositMint
        );
    }
    require!(slippage_bps < 5_000, LpDepositError::InvalidSlippage);

    // 严格报价模式：
    // - 链下 plan 决定本次应处于哪一种 zap mode
    // - 上链执行时只做校验，不再根据执行时价格自动 fallback/override plan
    let (sqrt_price_x64_now, tick_spacing) = {
        let pool_state = accounts.pool_state().load()?;
        (pool_state.sqrt_price_x64, pool_state.tick_spacing)
    };
    let quoted_mode = validate_quoted_mode_and_price(
        tick_lower_index,
        tick_upper_index,
        sqrt_price_x64_now,
        quoted_mode,
        quoted_sqrt_price_x64,
        slippage_bps,
    )?;

    let balance_0_before = accounts.signer_token0_account().amount;
    let balance_1_before = accounts.signer_token1_account().amount;

    // 校验：用户至少拥有本次允许的最大投入
    require!(
        balance_0_before >= amount_0_in,
        LpDepositError::InsufficientBalance
    );
    require!(
        balance_1_before >= amount_1_in,
        LpDepositError::InsufficientBalance
    );

    // 固定 quote 校验：任何模式下都不再允许链上重写 swap plan。
    if swap_amount_in == 0 {
        require!(swap_min_out == 0, LpDepositError::InvalidDepositAmount);
    } else if swap_input_is_token0 {
        require!(
            swap_amount_in <= amount_0_in,
            LpDepositError::InvalidDepositAmount
        );
    } else {
        require!(
            swap_amount_in <= amount_1_in,
            LpDepositError::InvalidDepositAmount
        );
    }

    match quoted_mode {
        QuotedZapMode::InRange => {}
        QuotedZapMode::OutOfRangeToken0Only => {
            if swap_amount_in > 0 {
                require!(!swap_input_is_token0, LpDepositError::InvalidDepositAmount);
            }
        }
        QuotedZapMode::OutOfRangeToken1Only => {
            if swap_amount_in > 0 {
                require!(swap_input_is_token0, LpDepositError::InvalidDepositAmount);
            }
        }
    }

    // remaining_accounts：用 programId 作为分隔符拆为两段（与现有逻辑一致）
    let sep = crate::ID;
    let sep_index = remaining_accounts
        .iter()
        .position(|a| a.key() == sep)
        .ok_or(LpDepositError::InvalidRemainingAccounts)?;
    let (swap_remaining_slice, rest) = remaining_accounts.split_at(sep_index);
    let action_remaining_slice = &rest[1..]; // 跳过分隔符本身

    // 执行主配平 swap（plan 指定，最多一次）
    let mut amount_0_max = amount_0_in;
    let mut amount_1_max = amount_1_in;
    let balance_0_pre_cpi: u64;
    let balance_1_pre_cpi: u64;

    if swap_amount_in > 0 {
        swap_v2_common(
            accounts,
            swap_amount_in,
            swap_min_out,
            0,
            swap_input_is_token0,
            swap_remaining_slice.to_vec(),
        )?;

        accounts.signer_token0_account().reload()?;
        accounts.signer_token1_account().reload()?;

        let balance_0_after_swap = accounts.signer_token0_account().amount;
        let balance_1_after_swap = accounts.signer_token1_account().amount;

        let spent_in = if swap_input_is_token0 {
            balance_0_before
                .checked_sub(balance_0_after_swap)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            balance_1_before
                .checked_sub(balance_1_after_swap)
                .ok_or(LpDepositError::MathOverflow)?
        };
        let amount_out_after = if swap_input_is_token0 {
            balance_1_after_swap
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            balance_0_after_swap
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?
        };

        // 若 token 有转账费，实际扣款可能 > swap_amount_in；必须确保不超过用户输入预算
        if swap_input_is_token0 {
            require!(
                spent_in <= amount_0_in,
                LpDepositError::InvalidDepositAmount
            );
        } else {
            require!(
                spent_in <= amount_1_in,
                LpDepositError::InvalidDepositAmount
            );
        }

        // 计算 CPI 可用的 max（= 输入预算经过 swap 后的可用额度）
        if swap_input_is_token0 {
            // token0 -> token1：token0 减少 swap_in，token1 增加 out
            amount_0_max = amount_0_in
                .checked_sub(spent_in)
                .ok_or(LpDepositError::MathOverflow)?;
            amount_1_max = amount_1_in
                .checked_add(amount_out_after)
                .ok_or(LpDepositError::MathOverflow)?;
        } else {
            // token1 -> token0：token1 减少 swap_in，token0 增加 out
            amount_1_max = amount_1_in
                .checked_sub(spent_in)
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
        swap_remaining: swap_remaining_slice,
        action_remaining: action_remaining_slice,
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
    return_mint: Option<Pubkey>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    computed_liquidity: u128,
    slippage_bps: u16,
    balance_0_pre_cpi: u64,
    balance_1_pre_cpi: u64,
    amount_0_max: u64,
    amount_1_max: u64,
    swap_remaining: &'info [AccountInfo<'info>],
    position_nft_mint: Pubkey,
) -> Result<(u64, u64)> {
    accounts.signer_token0_account().reload()?;
    accounts.signer_token1_account().reload()?;

    let balance_0_after_cpi = accounts.signer_token0_account().amount;
    let balance_1_after_cpi = accounts.signer_token1_account().amount;

    // CPI 实际花费（<= amount_max）
    let spent_0 = balance_0_pre_cpi
        .checked_sub(balance_0_after_cpi)
        .unwrap_or_else(|| {
            msg!(
                "WARN: spent_0 underflow: pre={}, after={}",
                balance_0_pre_cpi,
                balance_0_after_cpi
            );
            0
        });
    let spent_1 = balance_1_pre_cpi
        .checked_sub(balance_1_after_cpi)
        .unwrap_or_else(|| {
            msg!(
                "WARN: spent_1 underflow: pre={}, after={}",
                balance_1_pre_cpi,
                balance_1_after_cpi
            );
            0
        });

    let leftover_0 = amount_0_max.checked_sub(spent_0).unwrap_or_else(|| {
        msg!(
            "WARN: leftover_0 underflow: max={}, spent={}",
            amount_0_max,
            spent_0
        );
        0
    });
    let leftover_1 = amount_1_max.checked_sub(spent_1).unwrap_or_else(|| {
        msg!(
            "WARN: leftover_1 underflow: max={}, spent={}",
            amount_1_max,
            spent_1
        );
        0
    });

    // 可选：把非 return_mint 的剩余统一兑换成 return_mint
    let vault0 = accounts.vault_0_mint().key();
    let vault1 = accounts.vault_1_mint().key();
    // 默认：不指定 return_mint 时，两个币种的剩余都“原样退回”（留在用户 token account）
    let mut return_amount_0: u64 = leftover_0;
    let mut return_amount_1: u64 = leftover_1;

    if let Some(return_mint) = return_mint {
        require!(
            return_mint == vault0 || return_mint == vault1,
            LpDepositError::InvalidDepositMint
        );

        let vault0_is_usdc = vault0 == crate::consts::USDC_MIN;

        if return_mint == vault0 {
            // token0 作为返回币种
            let mut out_from_swap = 0u64;
            let mut kept_other = false;
            if leftover_1 > 0 {
                let before0 = accounts.signer_token0_account().amount;
                // refund swap 发生在主 swap / 加流动性之后，价格状态已被前序步骤合法更新；
                // 这里故意以当前池价作为结算锚点，而不复用主 swap 的链下报价锚点。
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
                let min = if vault0_is_usdc { min_out } else { leftover_1 };
                // 大于0.001 usdc 才swap，避免 swap 过小失败
                if min >= crate::consts::MIN_USDC_SWAP_AMOUNT {
                    swap_v2_common(
                        accounts,
                        leftover_1,
                        min_out,
                        0,
                        false,
                        swap_remaining.to_vec(),
                    )?;
                    accounts.signer_token0_account().reload()?;
                    out_from_swap = accounts
                        .signer_token0_account()
                        .amount
                        .checked_sub(before0)
                        .unwrap_or_else(|| {
                            msg!(
                                "WARN: out_from_swap underflow (to token0): after={}, before={}",
                                accounts.signer_token0_account().amount,
                                before0
                            );
                            0
                        });
                } else {
                    // 如果 min_out <= 0，则不进行 swap，把剩余的 token1 直接转账给 fee
                    // 如果剩余 mint 是 wSOL(native mint)，则不转给 fee（保持留在用户侧，函数末尾会统一 close/unwrap 成 SOL）
                    if accounts.vault_1_mint().key()
                        != anchor_spl::token::spl_token::native_mint::ID
                    {
                        let signer_ai = accounts.signer().to_account_info();
                        let fee_to_ai = accounts.fee_token1_account().to_account_info();
                        let mint_ai = accounts.vault_1_mint().to_account_info();
                        let mint_decimals = accounts.vault_1_mint().decimals;
                        let token_program_ai = accounts.token_program().to_account_info();
                        let token_program_2022_ai = accounts.token_program_2022().to_account_info();
                        let from_ai = accounts.signer_token1_account().to_account_info();
                        transfer_token_to_fee_accounts(
                            signer_ai,
                            from_ai,
                            fee_to_ai,
                            mint_ai,
                            mint_decimals,
                            token_program_ai,
                            Some(token_program_2022_ai),
                            leftover_1,
                        )?;
                        accounts.signer_token1_account().reload()?;
                    } else {
                        kept_other = true;
                    }
                }
            }
            return_amount_0 = leftover_0.checked_add(out_from_swap).unwrap_or_else(|| {
                msg!(
                    "WARN: return_amount_0 overflow: leftover_0={}, out_from_swap={}",
                    leftover_0,
                    out_from_swap
                );
                0
            });
            return_amount_1 = if kept_other { leftover_1 } else { 0 };
        } else {
            // token1 作为返回币种
            let mut out_from_swap = 0u64;
            let mut kept_other = false;
            if leftover_0 > 0 {
                let before1 = accounts.signer_token1_account().amount;
                // refund swap 发生在主 swap / 加流动性之后，价格状态已被前序步骤合法更新；
                // 这里故意以当前池价作为结算锚点，而不复用主 swap 的链下报价锚点。
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
                let min = if vault0_is_usdc { leftover_0 } else { min_out };
                if min >= crate::consts::MIN_USDC_SWAP_AMOUNT {
                    swap_v2_common(
                        accounts,
                        leftover_0,
                        min_out,
                        0,
                        true,
                        swap_remaining.to_vec(),
                    )?;
                    accounts.signer_token1_account().reload()?;
                    out_from_swap = accounts
                        .signer_token1_account()
                        .amount
                        .checked_sub(before1)
                        .unwrap_or_else(|| {
                            msg!(
                                "WARN: out_from_swap underflow (to token1): after={}, before={}",
                                accounts.signer_token1_account().amount,
                                before1
                            );
                            0
                        });
                } else {
                    // 如果 min_out <= 0，则不进行 swap，把剩余的 token0 直接转账给 fee
                    // 如果剩余 mint 是 wSOL(native mint)，则不转给 fee（保持留在用户侧，函数末尾会统一 close/unwrap 成 SOL）
                    if accounts.vault_0_mint().key()
                        != anchor_spl::token::spl_token::native_mint::ID
                    {
                        let signer_ai = accounts.signer().to_account_info();
                        let fee_to_ai = accounts.fee_token0_account().to_account_info();
                        let mint_ai = accounts.vault_0_mint().to_account_info();
                        let mint_decimals = accounts.vault_0_mint().decimals;
                        let token_program_ai = accounts.token_program().to_account_info();
                        let token_program_2022_ai = accounts.token_program_2022().to_account_info();
                        let from_ai = accounts.signer_token0_account().to_account_info();
                        transfer_token_to_fee_accounts(
                            signer_ai,
                            from_ai,
                            fee_to_ai,
                            mint_ai,
                            mint_decimals,
                            token_program_ai,
                            Some(token_program_2022_ai),
                            leftover_0,
                        )?;
                        accounts.signer_token0_account().reload()?;
                    } else {
                        kept_other = true;
                    }
                }
            }
            return_amount_1 = leftover_1.checked_add(out_from_swap).unwrap_or_else(|| {
                msg!(
                    "WARN: return_amount_1 overflow: leftover_1={}, out_from_swap={}",
                    leftover_1,
                    out_from_swap
                );
                0
            });
            return_amount_0 = if kept_other { leftover_0 } else { 0 };
        }
    }

    emit!(LpHandlerIncreaseLiquidityEvent {
        pool: accounts.pool_state().key(),
        position_nft_mint,
        principal_0: spent_0,
        principal_1: spent_1,
        token0_mint: vault0,
        token1_mint: vault1,
        tick_lower_index,
        tick_upper_index,
        liquidity: computed_liquidity,
        amount_0_in,
        amount_1_in,
        return_amount_0,
        return_amount_1,
    });

    // 这里必须先把 AccountInfo 拷贝出来，避免同时出现 &self / &mut self 的借用冲突
    let user_ai = accounts.signer().to_account_info();
    let user_token0_ai = { accounts.signer_token0_account().to_account_info() };
    let user_token1_ai = { accounts.signer_token1_account().to_account_info() };
    let token_program_ai = accounts.token_program().to_account_info();
    let token_program_2022_ai = accounts.token_program_2022().to_account_info();

    // 若池子某侧 mint 为 wSOL(native mint)，则强制把对应的 signer token account close 成 SOL。
    // 这样无论 `recipient` 是否等于 `signer`，用户最终都只会收到 SOL（不残留 wSOL token）。
    //
    // 注意：这里会 close 传入的 token account（不要求必须是 ATA）。
    // 调用方需要确保传入的就是“允许被 close 的 wSOL 账户”（通常为 wSOL ATA）。
    let wsol_mint_key = anchor_spl::token::spl_token::native_mint::ID;
    if accounts.vault_0_mint().key() == wsol_mint_key {
        unwrap_wsol_to_destination(
            user_ai.clone(),
            user_token0_ai.clone(),
            user_ai.clone(),
            token_program_ai.clone(),
            Some(token_program_2022_ai.clone()),
        )?;
    }
    if accounts.vault_1_mint().key() == wsol_mint_key {
        unwrap_wsol_to_destination(
            user_ai.clone(),
            user_token1_ai.clone(),
            user_ai.clone(),
            token_program_ai.clone(),
            Some(token_program_2022_ai),
        )?;
    }

    Ok((return_amount_0, return_amount_1))
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
    let payer_ai = accounts.signer().to_account_info();
    let amm_config_ai = accounts.amm_config().to_account_info();
    let pool_state_ai = accounts.pool_state().to_account_info();
    let observation_state_ai = accounts.observation_state().to_account_info();
    let token_program_ai = accounts.token_program().to_account_info();
    let token_program_2022_ai = accounts.token_program_2022().to_account_info();
    let memo_program_ai = accounts.memo_program().to_account_info();

    let user_token0_ai = accounts.signer_token0_account().to_account_info();
    let user_token1_ai = accounts.signer_token1_account().to_account_info();
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

/// unwrap wSOL：将一个 **native mint** 的 token account close，并把 lamports 打到指定 destination。
///
/// 重要：
/// - 该方法不会验证 token account 是否为 ATA，也不会检查其初始余额是否为 0；
///   调用方必须保证“关闭该账户不会误转走历史余额”。
/// - 对 wSOL(native mint) 来说，close 会把 lamports 退回 destination，效果等同 unwrap。
pub fn unwrap_wsol_to_destination<'info>(
    signer: AccountInfo<'info>,
    token_account: AccountInfo<'info>,
    destination: AccountInfo<'info>,
    token_program: AccountInfo<'info>,
    token_program_2022: Option<AccountInfo<'info>>,
) -> Result<()> {
    // token account 可能属于 SPL Token 或 Token-2022；按 owner 选择对应 close CPI。
    match token_program_2022.as_ref() {
        Some(tp22) if token_account.owner == tp22.key => {
            token_2022::close_account(CpiContext::new(
                tp22.to_account_info(),
                token_2022::CloseAccount {
                    account: token_account.to_account_info(),
                    destination: destination.to_account_info(),
                    authority: signer.to_account_info(),
                },
            ))
        }
        _ => token::close_account(CpiContext::new(
            token_program.to_account_info(),
            token::CloseAccount {
                account: token_account.to_account_info(),
                destination: destination.to_account_info(),
                authority: signer.to_account_info(),
            },
        )),
    }
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

    let mint_ai = mint.map(|m| (m.to_account_info(), m.decimals));
    transfer_token_common_accounts(
        signer.to_account_info(),
        from.to_account_info(),
        to.to_account_info(),
        mint_ai,
        token_program.to_account_info(),
        token_program_2022.map(|p| p.to_account_info()),
        amount,
    )?;

    Ok(())
}

/// 将 token account 的余额直接转给 fee 的 token account。
///
/// - 不做 wSOL(native mint) 的“转 lamports”特殊处理：这里只做纯 token transfer。
/// - 根据 `from` 的 owner 自动选择 SPL Token 或 Token-2022 的 CPI。
pub fn transfer_token_to_fee_accounts<'info>(
    signer: AccountInfo<'info>,
    from: AccountInfo<'info>,
    to: AccountInfo<'info>,
    mint: AccountInfo<'info>,
    mint_decimals: u8,
    token_program: AccountInfo<'info>,
    token_program_2022: Option<AccountInfo<'info>>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    transfer_token_common_accounts(
        signer,
        from,
        to,
        Some((mint, mint_decimals)),
        token_program,
        token_program_2022,
        amount,
    )
}

/// 通用 token 转账：根据 `from.owner` 选择 SPL Token 或 Token-2022 CPI。
///
/// - **SPL Token**: 使用 `token::transfer`
/// - **Token-2022**: 使用 `token_2022::transfer_checked`（必须提供 mint+decimals）
fn transfer_token_common_accounts<'info>(
    signer: AccountInfo<'info>,
    from: AccountInfo<'info>,
    to: AccountInfo<'info>,
    mint: Option<(AccountInfo<'info>, u8)>,
    token_program: AccountInfo<'info>,
    token_program_2022: Option<AccountInfo<'info>>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }

    if let Some(tp22) = token_program_2022 {
        if from.owner == &tp22.key() {
            let (mint_ai, decimals) = mint.ok_or(LpDepositError::InvalidDepositMint)?;
            token_2022::transfer_checked(
                CpiContext::new(
                    tp22,
                    token_2022::TransferChecked {
                        from,
                        to,
                        authority: signer,
                        mint: mint_ai,
                    },
                ),
                amount,
                decimals,
            )?;
            return Ok(());
        }
    }

    token::transfer(
        CpiContext::new(
            token_program,
            token::Transfer {
                from,
                to,
                authority: signer,
            },
        ),
        amount,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use raydium_amm_v3::libraries::get_sqrt_price_at_tick;

    #[test]
    fn validate_quoted_mode_accepts_matching_in_range_price() {
        let tick_lower_index = -100;
        let tick_upper_index = 100;
        let current_sqrt_price_x64 = get_sqrt_price_at_tick(0).unwrap();
        let quoted_sqrt_price_x64 = get_sqrt_price_at_tick(0).unwrap();

        let result = validate_quoted_mode_and_price(
            tick_lower_index,
            tick_upper_index,
            current_sqrt_price_x64,
            QuotedZapMode::InRange as u8,
            quoted_sqrt_price_x64,
            100,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn validate_quoted_mode_rejects_execution_mode_mismatch() {
        let tick_lower_index = -100;
        let tick_upper_index = 100;
        let current_sqrt_price_x64 = get_sqrt_price_at_tick(-120).unwrap();
        let quoted_sqrt_price_x64 = get_sqrt_price_at_tick(0).unwrap();

        let err = validate_quoted_mode_and_price(
            tick_lower_index,
            tick_upper_index,
            current_sqrt_price_x64,
            QuotedZapMode::InRange as u8,
            quoted_sqrt_price_x64,
            2_000,
        )
        .unwrap_err();

        assert_eq!(err, LpDepositError::QuotedModeMismatch.into());
    }

    #[test]
    fn validate_quoted_mode_rejects_price_below_floor() {
        let tick_lower_index = -100;
        let tick_upper_index = 100;
        let quoted_sqrt_price_x64 = get_sqrt_price_at_tick(0).unwrap();
        let current_sqrt_price_x64 = get_sqrt_price_at_tick(-600).unwrap();

        let err = validate_quoted_mode_and_price(
            tick_lower_index,
            tick_upper_index,
            current_sqrt_price_x64,
            QuotedZapMode::OutOfRangeToken0Only as u8,
            quoted_sqrt_price_x64,
            100,
        )
        .unwrap_err();

        assert_eq!(err, LpDepositError::QuotedPriceBelowMinimum.into());
    }
}
