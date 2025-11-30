use anchor_lang::prelude::*;
use raydium_amm_v3::libraries::{liquidity_math, U256};

pub fn calc_min_amount_out(
    swap_amount: u64,
    is_base_input: bool,
    sqrt_price_x64: u128,
    slippage_bps: u16,
) -> u64 {
    let amount_in = U256::from(swap_amount as u128);
    let sqrt_price = U256::from(sqrt_price_x64);

    // 计算价格 P = sqrt_price^2 / 2^64 (仍然是 Q64.64)
    let price_q128 = (sqrt_price * sqrt_price) >> 64; // Q64.64

    let slippage_factor = U256::from(10_000u128 - slippage_bps as u128);

    let out = if is_base_input {
        // token0 → token1
        //
        // out = in * price  (price 是 Q64.64，需要右移 64)
        let raw = (amount_in * price_q128) >> 64;

        // apply slippage
        (raw * slippage_factor) / U256::from(10_000u128)
    } else {
        // token1 → token0
        //
        // price = token1/token0
        // out = in / price = in * (1 / price)
        //
        // (1/price) = (2^64 / price_q128)
        let inv_price = ((U256::one() << 64) << 64) / price_q128;

        let raw = (amount_in * inv_price) >> 64;

        // apply slippage
        (raw * slippage_factor) / U256::from(10_000u128)
    };

    out.as_u64()
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
        let swap_amount = (deposit_amount).checked_sub(amount_0_needed).unwrap_or(0);
        return Ok((swap_amount, amount_1_needed));
    } else {
        let swap_amount = (deposit_amount).checked_sub(amount_1_needed).unwrap_or(0);
        return Ok((swap_amount, amount_0_needed));
    }
}
