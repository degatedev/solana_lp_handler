use anchor_lang::prelude::InterfaceAccount;
use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_2022::spl_token_2022::extension::{
    transfer_fee::TransferFeeConfig, BaseStateWithExtensions, StateWithExtensions,
};
use anchor_spl::token_interface::Mint;
use raydium_amm_v3::libraries::{liquidity_math, U256};

use crate::LpDepositError;

fn calculate_transfer_fee_from_config(
    transfer_fee_config: &TransferFeeConfig,
    epoch: u64,
    pre_fee_amount: u64,
) -> Result<u64> {
    // M1 修复：`calculate_epoch_fee` 内部已处理 min(raw_fee, maximum_fee)，
    // 无需对 MAX_FEE_BASIS_POINTS 做特殊分支。
    transfer_fee_config
        .calculate_epoch_fee(epoch, pre_fee_amount)
        .ok_or(LpDepositError::MathOverflow.into())
}

/// `require!` + 可选日志（不分配 heap）。
///
/// 说明：
/// - 使用 `msg!` 输出（不会分配 heap；避免 format!/String）
/// - 失败时会先打印，再返回 `anchor_lang::error!($error)`
///
/// 用法示例：
/// - `require_log!(cond, LpDepositError::SecurityNonWhitelistTokenAccount, "bad token account");`
/// - `require_log!(cond, LpDepositError::SecurityNonWhitelistTokenAccount, "bad token account",
///       some_pubkey_expr,
///       some_u64_expr,
///       some_bool_expr,
///   );`
#[macro_export]
macro_rules! require_log {
    ($invariant:expr, $error:expr) => {{
        if !($invariant) {
            return Err(anchor_lang::error!($error));
        }
    }};

    // format string + args (no heap): require_log!(cond, err, "a={} b={}", a, b)
    ($invariant:expr, $error:expr, $fmt:literal, $($args:expr),+ $(,)?) => {{
        if !($invariant) {
            anchor_lang::prelude::msg!($fmt, $($args),+);
            return Err(anchor_lang::error!($error));
        }
    }};
}

pub fn calc_min_amount_out(
    swap_amount: u64,
    input_is_token0: bool,
    sqrt_price_x64: u128,
    slippage_bps: u16,
    trade_fee_rate: u32, // Raydium: denominated in hundredths of a bip (10^-6), i.e. 1_000_000 = 100%
) -> Result<u64> {
    require!(slippage_bps <= 10_000, LpDepositError::InvalidSlippage);
    require!(trade_fee_rate <= 1_000_000, LpDepositError::MathOverflow);

    let amount_in = U256::from(swap_amount as u128);
    let sqrt_price = U256::from(sqrt_price_x64);

    // 计算价格 P = sqrt_price^2 / 2^64 (仍然是 Q64.64)
    let price_q128 = (sqrt_price * sqrt_price) >> 64; // Q64.64
    require!(!price_q128.is_zero(), LpDepositError::InvalidSqrtPrice);

    let slippage_factor = U256::from(10_000u128 - slippage_bps as u128);
    let fee_factor = U256::from(1_000_000u128 - trade_fee_rate as u128); // 10^-6

    let out_before_fee_and_slippage = if input_is_token0 {
        // token0 → token1
        //
        // out = in * price  (price 是 Q64.64，需要右移 64)
        let raw = (amount_in * price_q128) >> 64;
        raw
    } else {
        // token1 → token0
        //
        // price = token1/token0
        // out = in / price = in * (1 / price)
        //
        // (1/price) = (2^64 / price_q128)
        let inv_price = ((U256::one() << 64) << 64) / price_q128;

        let raw = (amount_in * inv_price) >> 64;
        raw
    };

    // LPH-010: 费用与滑点若分两次除法，会各自向下取整，产生复合精度损失。
    // 合并为一次整体除法，只截断一次，且仍保持 floor 语义（min_out 不会被抬高）。
    let denom = U256::from(1_000_000u128) * U256::from(10_000u128);
    let out = (out_before_fee_and_slippage * fee_factor * slippage_factor) / denom;

    // 防御：避免 U256 -> u64 截断
    require!(
        out <= U256::from(u64::MAX as u128),
        LpDepositError::MathOverflow
    );
    let out = out.as_u64();
    Ok(out)
}

/// 对给定 `amount` 应用滑点折扣（bps），返回 `amount * (1 - slippage)` 的结果。
/// 说明：
/// - 使用 `u128` 计算避免 `u64` 乘法溢出
/// - 采用向下取整（floor），保持“min_out 不会被抬高导致无谓失败”的语义
pub fn apply_slippage_bps_floor(amount: u64, slippage_bps: u16) -> Result<u64> {
    require!(slippage_bps <= 10_000, LpDepositError::InvalidSlippage);
    let factor = 10_000u128
        .checked_sub(slippage_bps as u128)
        .ok_or(LpDepositError::MathOverflow)?;
    let out = (amount as u128)
        .checked_mul(factor)
        .ok_or(LpDepositError::MathOverflow)?
        .checked_div(10_000u128)
        .ok_or(LpDepositError::MathOverflow)?;
    u64::try_from(out).map_err(|_| LpDepositError::MathOverflow.into())
}

/// 使用 Raydium liquidity_math 计算最优 swap 数量
pub fn calculate_optimal_swap_amount(
    deposit_amount: u64,
    is_token0: bool,
    current_tick: i32,
    tick_lower_index: i32,
    tick_upper_index: i32,
    sqrt_price_current_x64: u128,
    liquidity: u128,
) -> Result<(u64, u64)> {
    let (amount_0_needed, amount_1_needed) = liquidity_math::get_delta_amounts_signed(
        current_tick,
        sqrt_price_current_x64,
        tick_lower_index,
        tick_upper_index,
        // LPH-016: 避免 unwrap 导致 panic；改为显式错误传播。
        liquidity
            .try_into()
            .map_err(|_| error!(LpDepositError::MathOverflow))?,
    )?;
    msg!(
        "amount_0_needed={}, amount_1_needed={}",
        amount_0_needed,
        amount_1_needed
    );
    // 计算需要 swap 的数量
    if is_token0 {
        // 用户有 token0，需要换一部分成 token1
        // swap_amount = deposit_amount - amount_0_needed
        let swap_amount = deposit_amount
            .checked_sub(amount_0_needed)
            .ok_or(LpDepositError::MathOverflow)?;
        return Ok((swap_amount, amount_1_needed));
    } else {
        let swap_amount = deposit_amount
            .checked_sub(amount_1_needed)
            .ok_or(LpDepositError::MathOverflow)?;
        return Ok((swap_amount, amount_0_needed));
    }
}

pub fn get_transfer_fee_from_mint_info(mint_info: AccountInfo, pre_fee_amount: u64) -> Result<u64> {
    if *mint_info.owner == Token::id() {
        return Ok(0);
    }

    let mint_data = mint_info.try_borrow_data()?;
    let mint = StateWithExtensions::<anchor_spl::token_2022::spl_token_2022::state::Mint>::unpack(
        &mint_data,
    )?;

    let fee = if let Ok(transfer_fee_config) = mint.get_extension::<TransferFeeConfig>() {
        let epoch = Clock::get()?.epoch;
        calculate_transfer_fee_from_config(transfer_fee_config, epoch, pre_fee_amount)?
    } else {
        0
    };

    Ok(fee)
}

pub fn get_transfer_fee_for_amount(
    mint: &InterfaceAccount<Mint>,
    pre_fee_amount: u64,
) -> Result<u64> {
    get_transfer_fee_from_mint_info(mint.to_account_info(), pre_fee_amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_spl::token_2022::spl_token_2022::extension::transfer_fee::{
        TransferFee, TransferFeeConfig,
    };

    #[test]
    fn spl_mint_has_zero_transfer_fee() {
        let key = Pubkey::new_unique();
        let owner = anchor_spl::token::ID;
        let mut lamports = 0u64;
        let mut data = [];
        let mint_info = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            0,
        );

        assert_eq!(
            get_transfer_fee_from_mint_info(mint_info, 1_000).unwrap(),
            0
        );
    }

    #[test]
    fn transfer_fee_config_errors_when_epoch_fee_cannot_be_derived() {
        let result = calculate_transfer_fee_from_config(
            &test_transfer_fee_config(u16::MAX, u64::MAX),
            0,
            u64::MAX,
        );
        assert!(result.is_err());
    }

    fn test_transfer_fee_config(
        transfer_fee_basis_points: u16,
        maximum_fee: u64,
    ) -> TransferFeeConfig {
        TransferFeeConfig {
            transfer_fee_config_authority: Default::default(),
            withdraw_withheld_authority: Default::default(),
            withheld_amount: 0u64.into(),
            older_transfer_fee: TransferFee {
                epoch: 0u64.into(),
                maximum_fee: maximum_fee.into(),
                transfer_fee_basis_points: transfer_fee_basis_points.into(),
            },
            newer_transfer_fee: TransferFee {
                epoch: 0u64.into(),
                maximum_fee: maximum_fee.into(),
                transfer_fee_basis_points: transfer_fee_basis_points.into(),
            },
        }
    }
}

/// 计算“移除指定 liquidity”时应退回的 principal（不含手续费/奖励）数量
/// 说明：CLMM 中 principal 由当前价格与区间决定；奖励/手续费不属于 principal。
pub fn calculate_principal_amounts_for_liquidity(
    current_tick: i32,
    sqrt_price_current_x64: u128,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity: u128,
) -> Result<(u64, u64)> {
    let (amount_0, amount_1) = liquidity_math::get_delta_amounts_signed(
        current_tick,
        sqrt_price_current_x64,
        tick_lower_index,
        tick_upper_index,
        // LPH-016: 避免 unwrap 导致 panic；改为显式错误传播。
        liquidity
            .try_into()
            .map_err(|_| error!(LpDepositError::MathOverflow))?,
    )?;
    Ok((amount_0, amount_1))
}

pub fn derive_ata_address(
    owner: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
    ata_program: &Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        ata_program,
    )
    .0
}
