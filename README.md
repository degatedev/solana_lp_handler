# lp_handler 合约技术文档

 

## 目录

* [1. 合约概览与定位](#1-合约概览与定位)
* [2. ProgramId 与外部依赖](#2-programid-与外部依赖)
* [3. 总体数据流（两条指令）](#3-总体数据流两条指令)
* [4. 指令一：swap_and_deposit](#4-指令一swap_and_deposit)
  + [4.1 入口与参数](#41-入口与参数)
  + [4.2 账户模型与关键约束](#42-账户模型与关键约束)
  + [4.3 执行步骤](#43-执行步骤)
  + [4.4 remainingAccounts 规则](#44-remainingaccounts-规则)
  + [4.5 对照测试用例（调用约定）](#45-对照测试用例调用约定)
* [5. 指令二：decrease_liquidity](#5-指令二decrease_liquidity)
  + [5.1 入口与参数](#51-入口与参数)
  + [5.2 账户模型与关键约束](#52-账户模型与关键约束)
  + [5.3 remainingAccounts 分隔符协议（关键）](#53-remainingaccounts-分隔符协议关键)
  + [5.4 principal vs reward 与抽成模型（关键）](#54-principal-vs-reward-与抽成模型关键)
  + [5.5 兑换路径 convert_to_usdc=true（实现语义：兑换到目标币种）](#55-兑换路径-convert_to_usdctrue实现语义兑换到目标币种)
* [6. 事件（可观测性）](#6-事件可观测性)
* [7. 错误码与常见排查](#7-错误码与常见排查)
* [8. 信任边界与安全注意事项](#8-信任边界与安全注意事项)
* [9. 与测试用例的对应关系](#9-与测试用例的对应关系)
* [10. 交易示例](#10-交易示例)

## 参考代码位置

* 程序入口：`programs/lp_handler/src/lib.rs`
* 存入逻辑：`programs/lp_handler/src/instructions/swap_and_deposit.rs`
* 退出/领奖逻辑：`programs/lp_handler/src/instructions/decrease_liquidity.rs`
* 事件/错误/工具函数：`programs/lp_handler/src/state/`
* 调用方式示例：`tests/lp_deposit.test.ts`、`tests/lp_withdraw.test.ts`、`tests/lp_claim.test.ts`

---

## 1. 合约概览与定位

`lp_handler` 是一个 **Raydium CLMM（AmmV3）“组合交易/仓位操作”封装层**，把用户侧常见的两类动作做成稳定的链上入口：

* **存入侧**：单边资金 ->（必要时 swap）-> 开仓加流动性
* **退出/领奖侧**：减仓/领奖 ->（可选先 swap 归一）-> **只对奖励/手续费抽成**

核心目标：

* **降低集成复杂度**：把 Raydium CLMM 的多次 CPI 串成单条指令；
* **内置关键约束**：tick 区间、mint 合法性、fee 收款白名单、Position NFT ATA 推导一致性、remaining accounts 基础校验；
* **可观测性**：对 swap、加仓、减仓/领奖提供事件。

---

## 2. ProgramId 与外部依赖

### ProgramId

`lib.rs` 中 `declare_id!` 固定为主网 ProgramId（仓库注释说明本分支统一使用 mainnet 地址）。

### 外部依赖

* **Raydium CLMM**：`raydium_amm_v3`
  + CPI：`swap_v2`、`open_position_with_token22_nft`、`decrease_liquidity_v2`
* **Anchor SPL**：Token / Token2022 / ATA / Memo
* **合约内部工具**：`programs/lp_handler/src/state/utils.rs`
  + 滑点 `calc_min_amount_out`、`apply_slippage_bps_floor`
  + 最优 swap 比例估算 `calculate_optimal_swap_amount`
  + 本金计算 `calculate_principal_amounts_for_liquidity`
  + ATA 推导 `derive_ata_address`
* **抽成白名单**：`programs/lp_handler/src/state/consts.rs`

---

## 3. 总体数据流（两条指令）

#### `swap_and_deposit`

1. User 调用 `swap_and_deposit(deposit_amount, deposit_mint, tick_lower, tick_upper, liquidity, slippage_bps)`
2. 合约读取 `pool_state`，估算最优 swap 拆分（单边 -> 双边）
3. 如需 swap：CPI 调用 Raydium `swap_v2`（通过 tick arrays/bitmap 等 remaining accounts）
4. CPI 调用 Raydium `open_position_with_token22_nft`，铸造 Position NFT（Token2022）并开仓
5. 事件：`SwapExecutedEvent`（若发生 swap）、`IncreaseLiquidityEvent`

#### `decrease_liquidity`

1. User 调用 `decrease_liquidity(liquidity, mint_amount_0, mint_amount_1, swap_to_token_mint, slippage_bps, fee_percent, convert_to_usdc)`
2. 合约校验 `fee_owner` 白名单与 fee ATA
3. CPI 调用 Raydium `decrease_liquidity_v2`（奖励相关 remaining accounts 在分隔符之后）
4. 通过“余额增量 delta - principal_expected”拆分 reward；仅对 reward 抽成
5. 若 `convert_to_usdc=true`：可能再 CPI `swap_v2` 把 reward/principal 兑换到目标币种后再扣费
6. 事件：`DecreaseLiquidityEvent`
---

## 4. 指令一： `swap_and_deposit`

### 4.1 入口与参数

入口： `programs/lp_handler/src/lib.rs` -> `instructions::swap_and_deposit`

参数语义：

* `deposit_amount: u64`：单边投入数量（最小单位）
* `deposit_mint: Pubkey`：必须等于池子 token0 或 token1 的 mint
* `tick_lower_index/tick_upper_index: i32`：仓位 tick 区间，要求 `lower < upper`
* `liquidity: i128`（重要约定）
  + **仅用于“最优 swap 量估算”**
  + 实际开仓 CPI 时，合约固定传 `liquidity = 0` 给 Raydium（由 Raydium 根据 `amount_0_max/amount_1_max` 自动算实际 liquidity）
* `slippage_bps: u16`：滑点（bps），该指令中要求 `< 5000`

### 4.2 账户模型与关键约束

Accounts： `SwapAndDeposit<'info>`

关键约束点（非穷尽）：

* `raydium_clmm_program` 必须是 `raydium_amm_v3::ID`
* `pool_state.load()?.amm_config == amm_config.key()`
* `token_vault_0/1` 必须匹配 `pool_state.load()?.token_vault_0/1`
* `user_token0_account` 的 mint 必须是 `token_vault_0.mint`；`user_token1_account` 同理
* **Position NFT（Token2022）ATA 地址校验**：
  + `position_nft_account` 必须等于 `ATA(position_nft_owner, position_nft_mint, TOKEN_2022_PROGRAM_ID)`
  + 注意：合约只校验“地址推导是否正确”，允许 ATA 尚未创建（由 CPI 创建）

### 4.3 执行步骤

1. 参数校验：tick 区间、mint 合法性、slippage 上限、Position NFT ATA 地址正确
2. 读池状态：`tick_current`、`sqrt_price_x64`、`tick_spacing`
3. 估算最优 swap：`calculate_optimal_swap_amount(...)` 得到 `swap_amount_in`
4. 如需 swap：
   - 用 `calc_min_amount_out` 计算 `min_out` （滑点保护）
   - CPI 调用 Raydium `swap_v2`

   - 通过余额差计算实际产出，发 `SwapExecutedEvent`

5. 计算开仓 `amount_0_max/amount_1_max`（输入剩余 + swap 得到的对侧增量）
6. CPI 调用 Raydium `open_position_with_token22_nft`（合约固定传 `liquidity=0`）
7. 发 `IncreaseLiquidityEvent`

#### 资金去向与“保留策略”

`swap_and_deposit` 的资金使用方式不是“把两边都换得刚刚好再全部花光”，而是采用 **一边保留、一边尽量用完** 的策略（由实现逻辑直接决定）：

* **原始投入币种会保留一部分**：合约会计算 `swap_amount_min`，然后用 `deposit_amount - swap_amount_min` 作为该币种的 `amount_*_max`，也就是说 **不会把投入的那一边换光**。
* **swap 得到的对侧币种会“全量作为可用上限”提供给开仓**：对侧币种的 `amount_*_max` 直接取 swap 后的余额增量（`balance_after - balance_before`），等价于把 **本次 swap 得到的币全部作为最大可用量** 交给 Raydium 开仓 CPI。
  + 说明：`open_position_with_token22_nft` 接收的是 `amount_0_max/amount_1_max`（上限），Raydium 会按当时池子价格与区间计算实际消耗；如果因为舍入/价格变化导致没有用完某一边，上限中未消耗的部分会留在用户 ATA。

源码位置（便于核对）： `programs/lp_handler/src/instructions/swap_and_deposit.rs` 中 `amount_0_max/amount_1_max` 的计算处。

### 4.4 remainingAccounts 规则

`swap_and_deposit` 的 `ctx.remaining_accounts` **只用于 Raydium `swap_v2` **：

* 结构：`[bitmap_extension?] + [swap_tick_array_0..N]`
* 不允许把 open_position 需要的 tick arrays 混进 remaining（lower/upper 已作为固定账户传入）

合约对 remaining 做了轻量校验：

* 数量 `<= 32`
* 每个账户 `owner` 必须等于 `raydium_clmm_program`

### 4.5 对照测试用例（调用约定）

参考： `tests/lp_deposit.test.ts`

* 用 Raydium SDK 计算 swap 需要的 tick arrays，按顺序塞到 `remainingAccounts`
* `positionNftMint` 用 `Keypair.generate()` 生成并作为签名者
* `positionNftAccount` 采用 Token2022 的 ATA 推导（`getATAAddress(user, mint, TOKEN_2022_PROGRAM_ID)`）
* 交易层通常会提高 compute limit/price

---

## 5. 指令二： `decrease_liquidity`

### 5.1 入口与参数

入口： `programs/lp_handler/src/lib.rs` -> `instructions::decrease_liquidity`

参数语义：

* `liquidity: u128`
  + `>0`：典型 withdraw（本金 + 奖励）
  + `=0`：典型 claim（只领奖励；principal 视为 0）
* `mint_amount_0/mint_amount_1: u64`：传给 Raydium `decrease_liquidity_v2` 的参数（测试里多为 0）
* `swap_to_token_mint: Pubkey`：目标 mint（仅在 `convert_to_usdc=true` 时生效），必须是池子 token0 或 token1
* `slippage_bps: u16`：`<= 10000`
* `fee_percent: u16`：抽成比例（bps），`<= 10000`
* `convert_to_usdc: bool`：实现语义是“兑换到目标币种”（名称偏业务）

### 5.2 账户模型与关键约束

Accounts： `DecreaseLiquidity<'info>`

关键约束：

* `fee_owner` 必须命中 `FEE_OWNERS` 白名单（运行时校验）
* `fee_token0_account/fee_token1_account` 必须是 fee_owner 对应 mint 的 **ATA**
  + 并且会校验 `mint`、`owner`
  + ATA 推导时 token_program 使用 vault mint 账户的 `owner`（兼容 SPL Token / Token2022）

### 5.3 remainingAccounts 分隔符协议（关键）

`decrease_liquidity` 需要同时服务两类 CPI remaining：

* swap_v2 的 tick arrays/bitmap
* decrease_liquidity_v2 的奖励相关 remaining

合约使用 ** `lp_handler programId` 作为分隔符**：

* 在 `ctx.remaining_accounts` 中找到第一个 `pubkey == crate::ID` 的位置
* 分隔符前：`swap_remaining`
* 分隔符后（跳过分隔符本身）：`decrease_remaining`

缺分隔符会直接报 `InvalidRemainingAccounts` 。

对照测试（ `tests/lp_withdraw.test.ts` / `tests/lp_claim.test.ts` ）的 remainingAccounts 构造顺序：

1. push swap_remaining（bitmap_extension? + tick arrays）
2. push 分隔符：`{ pubkey: program.programId, ... }`
3. push decrease_remaining（可能含 bitmap_extension + 每个 reward 的三元组：poolRewardVault、ownerRewardVault、rewardMint）

### 5.4 principal vs reward 与抽成模型（关键）

该指令的设计关键点是：**只对 reward（手续费/奖励）抽成，不对 principal（本金）抽成**。

实现口径：

* 先 CPI 调用 Raydium `decrease_liquidity_v2`
* 通过用户 token0/token1 ATA 的 **余额增量**得到 `delta0/delta1`
* 计算 principal：
  + `liquidity == 0`：principal 视为 0（claim 语义）
  + `liquidity > 0`：用当前价格与区间估算 `principal_expected_0/1`
* `reward_gross = delta - principal_expected`（使用 `saturating_sub`，避免出现负数）
* 抽成：`integrator_fee = reward_gross * fee_percent / 10000`

### 5.5 兑换路径 convert_to_usdc=true（实现语义：兑换到目标币种）

当 `convert_to_usdc=true` ：

* `swap_to_token_mint` 决定目标边（token0 或 token1）
* 兑换流程被拆成两段：
  1. **先把 reward 的对侧部分换成目标币种**（便于“在目标币种里精确扣费”）
  2. 在目标币种里对 reward 抽成并 transfer 到 fee ATA
  3. **再把 principal 的对侧部分换成目标币种**
* `DecreaseLiquidityEvent` 在该模式下会把 principal/reward/fee **集中体现在目标币种一侧**，另一侧置 0

当 `convert_to_usdc=false` ：

* 不换币，对 token0/token1 各自 reward 分别抽成并分别 transfer 到 fee_token0/fee_token1 ATA

---

## 6. 事件（可观测性）

事件定义见 `programs/lp_handler/src/state/events.rs` ：

* `SwapExecutedEvent`：swap 输入/输出/最小输出、输入边、滑点、pool 等
* `IncreaseLiquidityEvent`：加仓 amount0/amount1、tick 区间、position nft mint 等
* `DecreaseLiquidityEvent`：principal、reward、integrator_fee（按“兑换后口径”或“双币口径”输出）

---

## 7. 错误码与常见排查

错误码见 `programs/lp_handler/src/state/errors.rs` ：

* `InvalidRemainingAccounts`
  + swap_and_deposit：remaining 超过 32 或 owner 不正确
  + decrease_liquidity：找不到分隔符或 swap_remaining 校验失败
* `InvalidPositionNftAccount`：Position NFT ATA 地址推导不匹配
* `InvalidDepositMint`：`deposit_mint`/`swap_to_token_mint` 不是池子 token0/token1
* `InvalidFeeOwner`/`InvalidFeeTokenAccount`：fee 白名单或 fee ATA 校验失败
* `InvalidTickRange`：tick 区间非法
* `InvalidSlippage`/`InvalidFeePercent`：参数越界
* `NoBalanceChange`：decrease 后两边增量都为 0（无可领/无可退）
* `MathOverflow`/`InvalidSqrtPrice`：运算或价格输入异常

排查建议：

* 首先确认 remainingAccounts 的组织规则（尤其 `decrease_liquidity` 的分隔符）
* 其次确认 fee_owner 与 fee ATA（mint/owner/推导 token program）是否正确
* 再看 Raydium CPI 的 log（tick arrays/bitmap 是否匹配当前 pool）

---

## 8. 信任边界与安全注意事项

### CPI 边界

合约强依赖 `raydium_amm_v3` 的账户校验与执行语义。本合约主要保证“上层协议与业务约束”：

* pool/vault/amm_config 的关联一致性
* Position NFT ATA 地址推导一致性
* remainingAccounts 的基础约束（数量与 owner）以及 decrease 的分隔符协议
* fee 白名单与 fee 收款账户强约束（避免“把 fee 转回自己绕过抽成”）

### remainingAccounts 风险面

* swap_v2 的 tick arrays/bitmap 由调用方提供：合约只做轻量 owner/数量校验，不解析其是否属于当前 pool；错误数据通常会在 Raydium CPI 失败。
* decrease_liquidity 的分隔符协议是逻辑正确性的关键：混放/缺失会直接失败或造成行为不符合预期。

### 滑点与价格读数

目前 `calc_min_amount_out` 使用 `sqrt_price_x64` 进行“基于现价的近似阈值”计算；如果价格剧烈波动或 tick 穿越较多，仍可能导致：

* 保护不够严（min_out 过低）
* 或过严导致失败（如果估算偏差）

为提升严谨性，已实现的做法是：**每次 CPI 调用 `swap_v2` 前重新读取 `pool_state.sqrt_price_x64` 再计算 `min_out` **（在 `decrease_liquidity` 的 reward swap 与 principal swap 前分别读取一次）。 

### reward-only 抽成的正确性假设

* principal 估算依赖当前 tick 与 sqrt_price 的一致性；极端情况下可能出现 `delta < principal_expected`，当前实现 `saturating_sub` 使 reward=0，避免抽到本金，但会影响 reward 统计口径。

---

## 9. 与测试用例的对应关系

* `tests/lp_deposit.test.ts`：存入（swap + open_position）与 swap remainingAccounts 构造示例
* `tests/lp_withdraw.test.ts`：减仓（含兑换 + close position）与 remainingAccounts 三段式组织示例
* `tests/lp_claim.test.ts`：领奖（`liquidity=0`）与 remainingAccounts 三段式组织示例

---

## 10.  交易示例

* withdraw : https://solscan.io/tx/4FP1XmPP16xFEUfnwhebY1GEskSzDbyQxNBCycpmUt6zPS1UVmdzkqB3P8J5C6BpgiXfYh2xz8KDSEG5AEdDcFRf
* deposit: https://solscan.io/tx/Tr5YioQQ5erjrvGBM87zmx5pmf6md3D8hiGdGcDXbPZye6rvSe891rBFJS38tCg3LGstgAtrWcF9dadiKDzrBys
* claim:  https://solscan.io/tx/4V91uuLWYLwho7M6a9qh6CBUDNUJaxCUyTG1mCLfV8MjU57QoeYSwVZEccQe4rgtgPbAgWwAMtfHKGoesmzn4Gc

 
