# DeGate LP Handler 补充审计修复报告

**日期**: 2026-03-19  
**审计来源**: `audit/2023-03-19-degate_audit_report_before_fix_review.md`

本文档仅覆盖本轮补充 fix review 中提到的 `M1` 、 `M2` 、 `M3` 、 `L2` 四项。

## 处理决定

| 问题 | 处理决定 | 说明 |
| --- | --- | --- |
| `M1` | `Fixed` | 当前 `calculate_transfer_fee_from_config` 已完全委托给 SPL Token-2022 的 `calculate_epoch_fee` ，不再保留 `MAX_FEE_BASIS_POINTS` 的特殊分支。 |
| `M2` | `Fixed` | 当前价格偏差校验已改为双向检查，不再只防价格单边下跌。 |
| `M3` | `Fixed` | 当前主 zap swap 的 `sqrt_price_limit_x64` 已按 `QuotedZapMode + swap direction` 同时覆盖 `in-range` 与 `out-of-range` 两类边界。 |
| `L2` | `Fixed` | 当前协议仅支持 USDC 配对池；在这一协议范围内，当前 dust 判断逻辑是准确的。 |

## `M1`

当前实现：

文件： `programs/lp_handler/src/state/utils.rs`

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

这和补充审计建议一致，不再重复实现 `min(raw_fee, maximum_fee)` 逻辑。

## `M2`

当前实现已改为双向价格偏差校验：

文件： `programs/lp_handler/src/instructions/zap_common.rs`

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

这已经覆盖价格向上、向下两个方向的偏移。

## `M3`

当前实现：

文件： `programs/lp_handler/src/instructions/zap_common.rs`

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
        // zero_for_one: limit 必须 < current_price
        if tick_price < current_sqrt_price_x64 { Ok(tick_price) } else { Ok(0) }
    } else {
        // not zero_for_one: limit 必须 > current_price
        if tick_price > current_sqrt_price_x64 { Ok(tick_price) } else { Ok(0) }
    }
}
```

其中：

* `InRange` 时，限制价格不要离开区间
* `OutOfRangeToken1Only` / `OutOfRangeToken0Only` 时，限制价格不要进入区间
* 若池价因定点数舍入已越过 tick 边界，传 0 放弃限价（边界已失效，`swap_min_out` 仍提供滑点保护）

## `L2`

本协议当前**仅支持 USDC 配对池**。  
在这一协议范围内，当前实现是自洽的：

* 如果目标币是 USDC，则使用 `swap_other_amount_threshold`，因为它是 USDC 计价的目标侧最小输出
* 如果目标币不是 USDC，则另一侧必然是 USDC，此时 `total_other_in` 本身就是 USDC 金额

当前实现：

文件： `programs/lp_handler/src/instructions/decrease_liquidity.rs`

```rust
let is_target_usdc = swap_to_token_mint == crate::consts::USDC_MINT;
let usdc_denominated_amount = if is_target_usdc {
    swap_other_amount_threshold
} else {
    total_other_in
};
```

以及：

```rust
fn should_skip_claim_only_dust_swap(
    principal_other_in: u64,
    usdc_denominated_amount: u64,
) -> bool {
    principal_other_in == 0 && usdc_denominated_amount < crate::consts::MIN_USDC_SWAP_AMOUNT
}
```

因此，本项当前定性为 `Fixed` 。
若未来协议范围扩展到非 USDC/非 USDC 池，当前实现不能直接沿用，需要增加 `mint -> threshold` 配置或其他等价机制。

---

## L01 回退说明：Cleanup swap remaining accounts 恢复为复用主 swap

**原始修复**: 将 remaining_accounts 从两段 `[swap, SEP, action]` 扩展为四段 `[swap, SEP, action, SEP, cleanup_token0, SEP, cleanup_token1]`，为 cleanup swap 提供独立的 tick arrays。

**回退原因**: 该修复在实际部署后引入了比原始问题更严重的回归：

1. **tick array 不匹配**: cleanup tick arrays 在链下基于主 swap 前的价格计算，但链上执行时主 swap 已大幅移动价格，导致 Raydium CPI 失败（`InvalidFirstTickArrayAccount`）
2. **tickArrayCache 覆盖不足**: 链下 `tickArrayCache` 仅覆盖当前 tick 附近的有限范围。尝试基于 post-swap 价格报价时，如果主 swap 将价格推到 cache 范围之外，则完全无法为 cleanup swap 计算有效的 tick arrays
3. **审计建议的前提不可行**: 审计原文指出"客户端必须基于预测的 post-main-swap + post-action 状态计算 swap_back_remaining"，但在实践中链下无法可靠预测 post-swap 池子状态

**复用主 swap tick arrays 的合理性**: 主 swap 的 tick arrays 覆盖了从 pre-swap tick 到 post-swap tick 的完整路径。cleanup swap 从 post-swap tick 出发，其起始 tick array 必然在主 swap 的 tick arrays 中。Raydium 的 swap 逻辑会扫描所有提供的 tick arrays 找到匹配的 start_tick_index，不要求顺序或方向一致。对于 cleanup swap 的小金额场景，覆盖范围充足。

**残余风险**: 仅在极端边界情况下（主 swap 恰好停在 tick array 边界且 cleanup 需要跨越到下一个 tick array）可能失败。此时交易回滚，用户资金不受影响，重试即可。概率极低，不涉及资金安全。

**当前状态**: `Acknowledged` — 回退为复用方案，remaining_accounts 恢复为两段 `[swap, SEP, action]`。
