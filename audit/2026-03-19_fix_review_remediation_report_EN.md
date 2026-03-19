# DeGate LP Handler Supplemental Audit Remediation Report

**Date**: 2026-03-19  
**Audit Source**: `audit/2023-03-19-degate_audit_report_before_fix_review.md`

This document only covers the four items raised in this supplemental fix review: `M1`, `M2`, `M3`, and `L2`.

## Decisions

| Issue | Decision | Notes |
| --- | --- | --- |
| `M1` | `Fixed` | `calculate_transfer_fee_from_config` now fully delegates to SPL Token-2022 `calculate_epoch_fee` and no longer keeps a special branch for `MAX_FEE_BASIS_POINTS`. |
| `M2` | `Fixed` | Price deviation validation is now symmetric and no longer protects only against one-sided price drops. |
| `M3` | `Fixed` | The main zap swap `sqrt_price_limit_x64` now covers both `in-range` and `out-of-range` boundaries based on `QuotedZapMode + swap direction`. |
| `L2` | `Fixed` | The protocol currently supports USDC-paired pools only; within that protocol scope, the current dust-threshold logic is accurate. |

## `M1`

Current implementation:

File: `programs/lp_handler/src/state/utils.rs`

```rust
fn calculate_transfer_fee_from_config(
    transfer_fee_config: &TransferFeeConfig,
    epoch: u64,
    pre_fee_amount: u64,
) -> Result<u64> {
    transfer_fee_config
        .calculate_epoch_fee(epoch, pre_fee_amount)
        .ok_or(LpDepositError::MathOverflow.into())
}
```

This matches the supplemental audit recommendation and no longer re-implements the `min(raw_fee, maximum_fee)` logic externally.

## `M2`

The current implementation uses a symmetric price deviation check:

File: `programs/lp_handler/src/instructions/zap_common.rs`

```rust
pub(crate) fn validate_price_from_quote(
    current_sqrt_price_x64: u128,
    quoted_sqrt_price_x64: u128,
    slippage_bps: u16,
) -> Result<()> {
    require!(slippage_bps <= 10_000, LpDepositError::InvalidSlippage);
    require!(quoted_sqrt_price_x64 > 0, LpDepositError::InvalidSlippage);

    let current_price =
        (U256::from(current_sqrt_price_x64) * U256::from(current_sqrt_price_x64)) >> 64;
    let quoted_price =
        (U256::from(quoted_sqrt_price_x64) * U256::from(quoted_sqrt_price_x64)) >> 64;

    let delta = if current_price >= quoted_price {
        current_price - quoted_price
    } else {
        quoted_price - current_price
    };

    require!(
        delta * U256::from(10_000u128) <= quoted_price * U256::from(slippage_bps as u128),
        LpDepositError::QuotedPriceBelowMinimum
    );

    Ok(())
}
```

This now covers both upward and downward price deviation.

## `M3`

Current implementation:

File: `programs/lp_handler/src/instructions/zap_common.rs`

```rust
fn derive_main_swap_price_limit(
    tick_lower_index: i32,
    tick_upper_index: i32,
    swap_input_is_token0: bool,
    mode: QuotedZapMode,
    current_sqrt_price_x64: u128,
) -> Result<u128> {
    use QuotedZapMode::*;
    let tick_price = match (swap_input_is_token0, mode) {
        (true, InRange) => get_sqrt_price_at_tick(tick_lower_index)?,
        (false, InRange) => get_sqrt_price_at_tick(tick_upper_index)?,
        (true, OutOfRangeToken1Only) => get_sqrt_price_at_tick(tick_upper_index)?,
        (false, OutOfRangeToken0Only) => get_sqrt_price_at_tick(tick_lower_index)?,
        _ => return err!(LpDepositError::InvalidDepositAmount),
    };
    if swap_input_is_token0 {
        // zero_for_one: limit must be < current_price
        if tick_price < current_sqrt_price_x64 { Ok(tick_price) } else { Ok(0) }
    } else {
        // not zero_for_one: limit must be > current_price
        if tick_price > current_sqrt_price_x64 { Ok(tick_price) } else { Ok(0) }
    }
}
```

Specifically:

* In `InRange`, the limit prevents price from leaving the position range
* In `OutOfRangeToken1Only` / `OutOfRangeToken0Only`, the limit prevents price from entering the range
* If the pool price has already crossed the tick boundary due to fixed-point rounding, 0 is passed to disable the limit (the boundary is already breached; `swap_min_out` still provides slippage protection)

## `L2`

The protocol currently **supports USDC-paired pools only**.  
Within that scope, the current implementation is internally consistent:

* If the target token is USDC, `swap_other_amount_threshold` is used because it is the target-side minimum output denominated in USDC
* If the target token is not USDC, the other side must be USDC, so `total_other_in` itself is already a USDC-denominated amount

Current implementation:

File: `programs/lp_handler/src/instructions/decrease_liquidity.rs`

```rust
let is_target_usdc = swap_to_token_mint == crate::consts::USDC_MINT;
let usdc_denominated_amount = if is_target_usdc {
    swap_other_amount_threshold
} else {
    total_other_in
};
```

And:

```rust
fn should_skip_claim_only_dust_swap(
    principal_other_in: u64,
    usdc_denominated_amount: u64,
) -> bool {
    principal_other_in == 0 && usdc_denominated_amount < crate::consts::MIN_USDC_SWAP_AMOUNT
}
```

Accordingly, this item is currently classified as `Fixed`.
If the protocol scope is later expanded to support non-USDC/non-USDC pools, this implementation should not be reused as-is and would need a `mint -> threshold` configuration or an equivalent mechanism.

---

## L01 Revert: Cleanup swap remaining accounts reverted to reusing main swap

**Original fix**: Extended remaining_accounts from two segments `[swap, SEP, action]` to four segments `[swap, SEP, action, SEP, cleanup_token0, SEP, cleanup_token1]`, providing independent tick arrays for the cleanup swap.

**Reason for revert**: The fix introduced regressions more severe than the original issue after deployment:

1. **Tick array mismatch**: Cleanup tick arrays were computed off-chain at the pre-main-swap price, but on-chain the main swap had already moved the price significantly, causing Raydium CPI failure (`InvalidFirstTickArrayAccount`)
2. **Insufficient tickArrayCache coverage**: The off-chain `tickArrayCache` only covers a limited range around the current tick. When attempting to quote at the post-swap price, the post-swap tick may fall outside the cache range, making it impossible to compute valid cleanup tick arrays
3. **Audit recommendation's prerequisite is infeasible**: The original audit states "the client must compute swap_back_remaining against the predicted post-main-swap and post-action state," but reliably predicting the post-swap pool state off-chain is not practical

**Why reusing main swap tick arrays works**: The main swap's tick arrays cover the full path from pre-swap tick to post-swap tick. The cleanup swap starts from the post-swap tick, so its starting tick array is guaranteed to be present in the main swap's tick arrays. Raydium's swap logic scans all provided tick arrays to find the matching `start_tick_index`, regardless of order or direction. For the small amounts involved in cleanup swaps, the coverage is sufficient.

**Residual risk**: Failure is possible only in an extreme edge case where the main swap stops exactly at a tick array boundary and the cleanup needs to cross into the next tick array. In that case the transaction reverts, user funds are unaffected, and a retry succeeds. The probability is very low and there is no fund-safety concern.

**Current status**: `Acknowledged` — reverted to reuse approach, remaining_accounts restored to two segments `[swap, SEP, action]`.
