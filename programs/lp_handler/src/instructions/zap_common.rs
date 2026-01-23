use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token;
use anchor_spl::token::Token;
use anchor_spl::token_2022::{self, Token2022};
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::libraries::{get_sqrt_price_at_tick, liquidity_math};
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{AmmConfig, ObservationState, PoolState};

use crate::{utils, LpDepositError, LpHandlerIncreaseLiquidityEvent};

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
    return_mint: Option<Pubkey>,
    tick_lower_index: i32,
    tick_upper_index: i32,
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

    // 上链执行时如果价格跑出区间：允许“单边投入”，并由合约自动把不需要的一侧换成需要的一侧（最多一次 swap）。
    // - sp <= sa：区间在现价上方 → 只需要 token0 → 若 amount_1_in>0，自动执行 token1->token0 全额兑换
    // - sp >= sb：区间在现价下方 → 只需要 token1 → 若 amount_0_in>0，自动执行 token0->token1 全额兑换
    // - sa < sp < sb：区间跨现价 → 走链下 plan（swap_amount_in/min_out/方向）
    let (sqrt_price_x64_now, tick_spacing) = {
        let pool_state = accounts.pool_state().load()?;
        (pool_state.sqrt_price_x64, pool_state.tick_spacing)
    };
    let sa = get_sqrt_price_at_tick(tick_lower_index)?;
    let sb = get_sqrt_price_at_tick(tick_upper_index)?;

    // 最终实际执行的 swap（可能来自链下 plan，也可能因“出区间”被合约覆盖）
    let trade_fee_rate = accounts.amm_config().trade_fee_rate;
    let mut exec_swap_amount_in = swap_amount_in;
    let mut exec_swap_min_out = swap_min_out;
    let mut exec_swap_input_is_token0 = swap_input_is_token0;

    let out_of_range = sqrt_price_x64_now <= sa || sqrt_price_x64_now >= sb;
    if sqrt_price_x64_now <= sa {
        // 只需要 token0：把 token1 全换成 token0
        exec_swap_input_is_token0 = false;
        exec_swap_amount_in = amount_1_in;
        exec_swap_min_out = if exec_swap_amount_in > 0 {
            utils::calc_min_amount_out(
                exec_swap_amount_in,
                exec_swap_input_is_token0,
                sqrt_price_x64_now,
                slippage_bps,
                trade_fee_rate,
            )?
        } else {
            0
        };
    } else if sqrt_price_x64_now >= sb {
        // 只需要 token1：把 token0 全换成 token1
        exec_swap_input_is_token0 = true;
        exec_swap_amount_in = amount_0_in;
        exec_swap_min_out = if exec_swap_amount_in > 0 {
            utils::calc_min_amount_out(
                exec_swap_amount_in,
                exec_swap_input_is_token0,
                sqrt_price_x64_now,
                slippage_bps,
                trade_fee_rate,
            )?
        } else {
            0
        };
    }

    let balance_0_before = accounts.user_token0_account().amount;
    let balance_1_before = accounts.user_token1_account().amount;

    // 校验：用户至少拥有本次允许的最大投入
    require!(
        balance_0_before >= amount_0_in,
        LpDepositError::InsufficientBalance
    );
    require!(
        balance_1_before >= amount_1_in,
        LpDepositError::InsufficientBalance
    );

    // 仅在“区间跨现价”时校验链下 plan（出区间时 swap 会被合约覆盖）
    if !out_of_range {
        // swap_in 不能超过输入预算；swap=0 时 min_out 必须为 0
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
    }

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

    // 执行主配平 swap（plan 指定，最多一次）
    let mut amount_0_max = amount_0_in;
    let mut amount_1_max = amount_1_in;
    let balance_0_pre_cpi: u64;
    let balance_1_pre_cpi: u64;

    if exec_swap_amount_in > 0 {
        swap_v2_common(
            accounts,
            exec_swap_amount_in,
            exec_swap_min_out,
            0,
            exec_swap_input_is_token0,
            swap_remaining.clone(),
        )?;

        accounts.user_token0_account().reload()?;
        accounts.user_token1_account().reload()?;

        let balance_0_after_swap = accounts.user_token0_account().amount;
        let balance_1_after_swap = accounts.user_token1_account().amount;

        let spent_in = if exec_swap_input_is_token0 {
            balance_0_before
                .checked_sub(balance_0_after_swap)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            balance_1_before
                .checked_sub(balance_1_after_swap)
                .ok_or(LpDepositError::MathOverflow)?
        };
        let amount_out_after = if exec_swap_input_is_token0 {
            balance_1_after_swap
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            balance_0_after_swap
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?
        };

        // 若 token 有转账费，实际扣款可能 > swap_amount_in；必须确保不超过用户输入预算
        if exec_swap_input_is_token0 {
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
        if exec_swap_input_is_token0 {
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
    return_mint: Option<Pubkey>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    computed_liquidity: u128,
    slippage_bps: u16,
    balance_0_pre_cpi: u64,
    balance_1_pre_cpi: u64,
    amount_0_max: u64,
    amount_1_max: u64,
    swap_remaining: Vec<AccountInfo<'info>>,
    position_nft_mint: Pubkey,
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

    // 可选：把非 return_mint 的剩余统一兑换成 return_mint
    let vault0 = accounts.vault_0_mint().key();
    let vault1 = accounts.vault_1_mint().key();
    let mut return_amount: u64 = 0;

    if let Some(return_mint) = return_mint {
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
                if min_out > 0 {
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
                if min_out > 0 {
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
            }
            return_amount = leftover_1.checked_add(out_from_swap).unwrap_or(0);
        }
    }

    emit!(LpHandlerIncreaseLiquidityEvent {
        user: accounts.user().key(),
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
        return_mint,
        return_amount,
    });

    // 这里必须先把 AccountInfo 拷贝出来，避免同时出现 &self / &mut self 的借用冲突
    let user_ai = accounts.user().to_account_info();
    let user_token0_ai = { accounts.user_token0_account().to_account_info() };
    let user_token1_ai = { accounts.user_token1_account().to_account_info() };
    let token_program_ai = accounts.token_program().to_account_info();
    let token_program_2022_ai = accounts.token_program_2022().to_account_info();
    let associated_token_program_ai = accounts.associated_token_program().to_account_info();

    unwrap_wsol_ata_if_needed(
        user_ai,
        [user_token0_ai, user_token1_ai],
        token_program_ai,
        Some(token_program_2022_ai),
        associated_token_program_ai,
    )?;

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

// unwrap wSOL ATA：仅当传入的 token account 确实是 user 的 wSOL ATA 时才执行关闭（否则跳过）
pub fn unwrap_wsol_ata_if_needed<'info>(
    user: AccountInfo<'info>,
    token_accounts: [AccountInfo<'info>; 2],
    token_program: AccountInfo<'info>,
    token_program_2022: Option<AccountInfo<'info>>,
    associated_token_program: AccountInfo<'info>,
) -> Result<()> {
    // wSOL = SPL Token native mint
    let wsol_mint_key = anchor_spl::token::spl_token::native_mint::ID;

    // native(wSOL) 账户允许在 amount != 0 时 close：lamports 会退回 destination（这里是 user），效果等同 unwrap
    for token_acc in token_accounts.iter() {
        // 仅关闭 user 的 ATA；不是就跳过（不报错）
        let token_program_for_ata = match token_program_2022.as_ref() {
            Some(tp22) if token_acc.owner == tp22.key => tp22.key(),
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

        match token_program_2022.as_ref() {
            Some(tp22) if token_acc.owner == tp22.key => {
                token_2022::close_account(CpiContext::new(
                    tp22.to_account_info(),
                    token_2022::CloseAccount {
                        account: token_acc.to_account_info(),
                        destination: user.to_account_info(),
                        authority: user.to_account_info(),
                    },
                ))?;
            }
            _ => {
                token::close_account(CpiContext::new(
                    token_program.to_account_info(),
                    token::CloseAccount {
                        account: token_acc.to_account_info(),
                        destination: user.to_account_info(),
                        authority: user.to_account_info(),
                    },
                ))?;
            }
        }
    }

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
