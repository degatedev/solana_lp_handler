M1: Transfer fee aware token handling.
In the new `calculate_transfer_fee_from_config` function, `transfer_fee.maximum_fee` is returned if the `TransferFeeConfig` sets the fee to `MAX_FEE_BASIS_POINTS` , whereas the SPL Token 2022 behaviour returns `min(raw_fee, maximum_fee)` . We think this code block

```rust
if u16::from(transfer_fee.transfer_fee_basis_points) == MAX_FEE_BASIS_POINTS {
        Ok(u64::from(transfer_fee.maximum_fee))
    } else {
        transfer_fee_config
            .calculate_epoch_fee(epoch, pre_fee_amount)
            .ok_or(LpDepositError::MathOverflow.into())
    }
```

should be consolidated into

```rust
transfer_fee_config
    .calculate_epoch_fee(epoch, pre_fee_amount)
    .ok_or(LpDepositError::MathOverflow.into())
```

because the `calculate_epoch_fee` itself does the max fee comparison.

M2: Slippage on prepare_zap_plan_and_swap_if_needed
The `validate_price_floor_from_quote` only protects against one directional price movement (price dropping) which would be ineffective if swapping from token1 to token0 (price rising). We recommend adding a two-sided bounds check to protect price movement on both sides.

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
        LpDepositError::QuotedPriceDeviation
    );

    Ok(())
}
```

M3: Enforcing price boundaries after swap
The price boundaries are only enforced on swaps to prevent in-range -> out-of-range. We recommend checking both cases including out-of-range -> in-range using the newly added mode.

```rust
fn derive_main_swap_price_limit(
      tick_lower_index: i32,
      tick_upper_index: i32,
      swap_input_is_token0: bool,
      mode: QuotedZapMode,
  ) -> Result<u128> {
      match (swap_input_is_token0, mode) {
          // InRange: prevent exiting the range
          (true, InRange) => get_sqrt_price_at_tick(tick_lower_index),
          (false, InRange) => get_sqrt_price_at_tick(tick_upper_index),
          // OutOfRange: prevent entering the range
          (true, OutOfRangeToken1Only) => get_sqrt_price_at_tick(tick_upper_index),
          (false, OutOfRangeToken0Only) => get_sqrt_price_at_tick(tick_lower_index),
          _ => unreachable!(), // other combos blocked by mode validation
      }
  }
```

L2: Comparing arbitrary token mints against USDC threshold.

For this issue we are wondering if your protocol plans to support only USDC-paired pools e.g (USDC/SOL). It seems like the intended logic is to account for dust only arising from pools involving USDC. We would suggest two ways to move forward here:
1) If you plan to only support USDC paired pools, adjust the fixes to compare the dust against a USDC threshold on both sides(the current fix only checks one side and is missing the case where `min = total_other_in` ). We will change this finding from an issue to an assumption that the protocol only support USDC paired pools.
2) If you plan to support pools in which both tokens are non-USDC, you can have the dust logic contained in a check that the pool contains a USDC-side. Otherwise, in order to check for the correct dust amount for arbitrary tokens, we think having a mapping of `token_id` -> `threshold` maintained by the protocol admin to be the best solution here.
