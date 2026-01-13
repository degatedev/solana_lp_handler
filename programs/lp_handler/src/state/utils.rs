use anchor_lang::prelude::*;
use raydium_amm_v3::libraries::{liquidity_math, U256};

use crate::LpDepositError;

pub fn calc_min_amount_out(
    swap_amount: u64,
    is_base_input: bool,
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

    let out_before_fee_and_slippage = if is_base_input {
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

    // apply Raydium trade fee (10^-6) then slippage (bps)
    let out_after_fee = (out_before_fee_and_slippage * fee_factor) / U256::from(1_000_000u128);
    let out = (out_after_fee * slippage_factor) / U256::from(10_000u128);

    // 防御：避免 U256 -> u64 截断
    require!(
        out <= U256::from(u64::MAX as u128),
        LpDepositError::MathOverflow
    );
    Ok(out.as_u64())
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
    liquidity: i128,
) -> Result<(u64, u64)> {
    let (amount_0_needed, amount_1_needed) = liquidity_math::get_delta_amounts_signed(
        current_tick,
        sqrt_price_current_x64,
        tick_lower_index,
        tick_upper_index,
        liquidity,
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

/// 计算“移除指定 liquidity”时应退回的 principal（不含手续费/奖励）数量
/// 说明：CLMM 中 principal 由当前价格与区间决定；奖励/手续费不属于 principal。
pub fn calculate_principal_amounts_for_liquidity(
    current_tick: i32,
    sqrt_price_current_x64: u128,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity: u128,
) -> Result<(u64, u64)> {
    let liquidity_i128 = i128::try_from(liquidity).map_err(|_| LpDepositError::MathOverflow)?;
    let (amount_0, amount_1) = liquidity_math::get_delta_amounts_signed(
        current_tick,
        sqrt_price_current_x64,
        tick_lower_index,
        tick_upper_index,
        liquidity_i128,
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
