# open_position_without_swap / increase_liquidity_without_swap 指令设计文档

日期：2026-04-22（实现版本更新）

## 背景

现有 `swap_and_deposit` 和 `increase_liquidity` 指令均会执行"配平 swap"——先将用户输入的单币或双币兑换成池子所需比例，再调用 Raydium CLMM 开仓/追加流动性。

部分场景下，用户已在链下精确计算好双币比例，不希望合约再做任何 swap（主配平 swap 和 cleanup swap 均不执行），剩余双币原样退回。本文档描述为此新增的两个独立指令：

- **`open_position_without_swap`** — 双币直接开仓
- **`increase_liquidity_without_swap`** — 双币直接追加流动性

> 原设计为单指令 `deposit_without_swap`（通过 `nft_mint: Option<Pubkey>` 区分开仓/追加），最终拆分为两个独立指令，以便对每种场景的账户做类型安全约束。

---

## 目标

- 完全不执行任何 swap（主配平 swap 和 cleanup swap 均跳过）
- 不支持 `return_mint`，剩余双币原样退回用户
- 安全层（`secure_entrypoint` 宏）、`recipient` 逻辑、事件发射与现有指令完全一致
- 开仓与追加分为两个独立指令，账户结构各自最优

---

## 一、open_position_without_swap（开仓）

### 指令参数

```rust
pub fn open_position_without_swap(
    ctx: Context<OpenPositionWithoutSwap>,
    amount_0_in: u64,        // 用户愿意投入的 token0 上限
    amount_1_in: u64,        // 用户愿意投入的 token1 上限
    tick_lower_index: i32,   // 价格区间下边界
    tick_upper_index: i32,   // 价格区间上边界
    liquidity: u128,         // 链下预计算的期望 liquidity
    slippage_bps: u16,       // 容忍偏差（链上重算 vs 链下），单位基点
) -> Result<()>
```

### 账户结构 `OpenPositionWithoutSwap`

```
公共账户（与现有指令相同，但无 amm_config / observation_state / memo_program）：
  raydium_clmm_program    // Raydium CLMM program，地址约束
  recipient               // 第二个账户，安全层要求；NFT owner / 退款目标
  signer                  // 签名者 / 支付者
  signer_token0_account   // authority = signer
  signer_token1_account   // authority = signer
  refund_token0_account   // authority = recipient
  refund_token1_account   // authority = recipient
  pool_state              // mut
  fee_owner               // SystemAccount（安全层白名单校验；两个新指令不收取手续费）
  token_vault_0           // constraint = pool_state.token_vault_0
  token_vault_1           // constraint = pool_state.token_vault_1
  rent
  system_program
  token_program
  associated_token_program
  token_program_2022
  security_config         // seeds+bump 校验
  vault_0_mint
  vault_1_mint

Position 相关账户：
  position_nft_mint       // Signer<'info>（新 NFT mint keypair，必须签名）
  position_nft_account    // UncheckedAccount（NFT 将 mint 到的 token account）
  protocol_position       // UncheckedAccount（已废弃，兼容保留）
  personal_position       // UncheckedAccount（由 Raydium open_position CPI 初始化）
  tick_array_lower        // UncheckedAccount（可未初始化；地址在本程序内校验）
  tick_array_upper        // UncheckedAccount（可未初始化；地址在本程序内校验）
```

> `tick_array_lower` / `tick_array_upper` 使用 `UncheckedAccount` 的原因：开仓时这两个账户可能尚未初始化，Anchor 无法通过类型约束验证。程序内部通过 `validate_open_position_tick_array_keys` 手动校验 PDA 地址。

### remaining_accounts 格式

格式与现有指令一致，分隔符之前不放任何 swap 账户：

```
[separator: program.programId]
[optional: tick_array_bitmap_extension（若 tick range 溢出默认 bitmap）]
[action_tick_arrays...]
```

- `secure_entrypoint!` 安全层要求 remaining_accounts 中恰好出现 1 次分隔符，无论是否有 swap 均需遵守
- 链上调用 `split_remaining_accounts` 丢弃分隔符前的空段，只把分隔符后的 action 部分传给 Raydium CPI

---

## 二、increase_liquidity_without_swap（追加）

### 指令参数

```rust
pub fn increase_liquidity_without_swap(
    ctx: Context<IncreaseLiquidityWithoutSwap>,
    amount_0_in: u64,   // 用户愿意投入的 token0 上限
    amount_1_in: u64,   // 用户愿意投入的 token1 上限
    liquidity: u128,    // 链下预计算的期望 liquidity
    slippage_bps: u16,  // 容忍偏差（链上重算 vs 链下），单位基点
) -> Result<()>
```

> 无 `tick_lower_index` / `tick_upper_index` 参数，链上直接从 `personal_position` 读取。

### 账户结构 `IncreaseLiquidityWithoutSwap`

```
公共账户（与 open_position_without_swap 相同）：
  raydium_clmm_program
  recipient
  signer
  signer_token0_account
  signer_token1_account
  refund_token0_account
  refund_token1_account
  pool_state
  fee_owner
  token_vault_0 / token_vault_1
  rent / system_program / token_program / associated_token_program / token_program_2022
  security_config
  vault_0_mint / vault_1_mint

Position 相关账户（类型安全约束，无需手动校验）：
  position_nft_account    // InterfaceAccount<TokenAccount>
                          // constraint: mint == personal_position.nft_mint
                          // constraint: amount == 1
                          // token::authority = signer
  personal_position       // Account<PersonalPositionState>
                          // constraint: pool_id == pool_state.key()
  protocol_position       // UncheckedAccount（已废弃，兼容保留）
  tick_array_lower        // AccountLoader<TickArrayState>（必须已初始化）
                          // constraint: pool_id == pool_state.key()
  tick_array_upper        // AccountLoader<TickArrayState>（必须已初始化）
                          // constraint: pool_id == pool_state.key()
```

> 追加流动性时 tick array 必须已初始化，因此使用 `AccountLoader<TickArrayState>` 配合 Anchor constraint 自动完成校验，无需手动 PDA 验证。

### remaining_accounts 格式

同 `open_position_without_swap`，分隔符之前为空，格式：`[separator] [optional: bitmap_extension] [action_tick_arrays...]`

---

## 三、公共执行逻辑

两个指令共享 `zap_common.rs` 中的三个公共函数：

### `compute_and_validate_liquidity`

```rust
pub fn compute_and_validate_liquidity(
    pool_state: &AccountLoader<PoolState>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    vault_0_mint: &InterfaceAccount<Mint>,
    vault_1_mint: &InterfaceAccount<Mint>,
    amount_0_in: u64,
    amount_1_in: u64,
    quoted_liquidity: u128,
    slippage_bps: u16,
) -> Result<(u128, u16)>  // (computed_liquidity, tick_spacing)
```

- 从 pool_state 读取 `sqrt_price_x64` 和 `tick_spacing`
- 扣除 transfer fee 后计算实际可用金额
- 调用 `get_liquidity_from_amounts` 链上重算 liquidity
- 校验偏差：`|computed - quoted| * 10_000 <= quoted * slippage_bps`
- 返回 `(computed_liquidity, tick_spacing)`

### `settle_deposit_without_swap`

CPI 完成后的统一结算：
1. 对比 reload 前后余额，计算 `spent_0` / `spent_1` / `leftover_0` / `leftover_1`
2. 发出 `LpHandlerIncreaseLiquidityEvent` 事件
3. 非 wSOL 剩余：若 `refund_tokenX_account != signer_tokenX_account`，transfer leftover 给 refund 账户
4. wSOL 剩余：close token account，lamports 发往 `recipient`（若 `signer == recipient` 则发 signer）

### `validate_liquidity_slippage`

底层校验函数，被 `compute_and_validate_liquidity` 调用：

```
|computed - quoted| * 10_000 <= quoted * slippage_bps
```

---

## 四、去掉的参数（与现有 swap 指令相比）

| 参数 | 去掉原因 |
|------|----------|
| `return_mint` | 不做 cleanup swap，剩余双币直接退回 |
| `quoted_mode` | 无 swap，无需报价模式校验 |
| `quoted_sqrt_price_x64` | 无需价格锚定，改为 liquidity 偏差校验 |
| `swap_amount_in` | 无 swap |
| `swap_min_out` | 无 swap |
| `swap_input_is_token0` | 无 swap |
| `nft_mint` | 开仓/追加拆分为两个独立指令，不再需要 Option |

---

## 五、recipient 逻辑

与现有指令完全一致：

| 场景 | recipient 的作用 |
|------|----------------|
| 开仓 | `position_nft_owner` = recipient（NFT 归属 recipient） |
| 追加 | 无（NFT 已存在，owner 不变） |
| 退款 token | `refund_tokenX_account.authority` = recipient |
| wSOL unwrap | lamports 目标：signer == recipient 时发 signer，否则发 recipient |
| 安全层 | recipient 为第二个账户，做 authority/delegate/close_authority 对账 |

---

## 六、事件

复用 `LpHandlerIncreaseLiquidityEvent`：

```
pool, position_nft_mint, principal_0, principal_1,
token0_mint, token1_mint, tick_lower_index, tick_upper_index,
liquidity (= computed_liquidity),
amount_0_in, amount_1_in,
return_amount_0 (= leftover_0),
return_amount_1 (= leftover_1)
```

---

## 七、安全层集成

在 `lib.rs` 中通过 `secure_entrypoint!` 宏包装，与其他指令一致：

```rust
pub fn open_position_without_swap(...) -> Result<()> {
    secure_entrypoint!(ctx, signer=..., recipient=..., fee_owner=..., pool_state=...,
        body = instructions::open_position_without_swap(ctx, ...))
}

pub fn increase_liquidity_without_swap(...) -> Result<()> {
    secure_entrypoint!(ctx, signer=..., recipient=..., fee_owner=..., pool_state=...,
        body = instructions::increase_liquidity_without_swap(ctx, ...))
}
```

---

## 八、文件变更清单

| 文件 | 变更 |
|------|------|
| `src/instructions/open_position_without_swap.rs` | 新建，含账户结构体、指令实现、tick array 校验、单元测试 |
| `src/instructions/increase_liquidity_without_swap.rs` | 新建，含账户结构体、指令实现 |
| `src/instructions/zap_common.rs` | 新增 `compute_and_validate_liquidity`、`validate_liquidity_slippage`、`settle_deposit_without_swap`、`WithoutSwapSettleAccounts`、`WithoutSwapEventParams` |
| `src/instructions/mod.rs` | 注册两个新模块，移除旧 `deposit_without_swap` |
| `src/lib.rs` | 新增两个入口函数（secure_entrypoint 包装），移除旧 `deposit_without_swap` |
| `src/state/errors.rs` | 新增 `LiquiditySlippageExceeded` 错误码 |
| `tests/open_position_without_swap.test.ts` | 新建 JS 集成测试 |
| `tests/increase_liquidity_without_swap.test.ts` | 新建 JS 集成测试 |

---

## 九、不变更的内容

- `decrease_liquidity.rs`、`swap_and_deposit.rs`、`increase_liquidity.rs`：均不改动
- 安全层（`security/mod.rs`）：不改动
- 现有测试：不改动
