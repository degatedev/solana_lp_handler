# lp_handler 合约技术文档

 

## 目录

* [1. 合约概览与定位](#1-合约概览与定位)
* [2. ProgramId 与外部依赖](#2-programid-与外部依赖)
* [3. 总体数据流（三条核心指令）](#3-总体数据流三条核心指令)
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
  + [5.5 兑换路径 convert_to_target_mint=true（实现语义：兑换到目标币种）](#55-兑换路径-convert_to_target_minttrue实现语义兑换到目标币种)
  + [5.6 Dust 规则（关键）](#56-dust-规则关键)
* [6. 事件（可观测性）](#6-事件可观测性)
* [7. 错误码与常见排查](#7-错误码与常见排查)
* [8. 信任边界与安全注意事项](#8-信任边界与安全注意事项)
* [9. 与测试用例的对应关系](#9-与测试用例的对应关系)
* [10. 交易示例](#10-交易示例)

## 参考代码位置

* 程序入口：`programs/lp_handler/src/lib.rs`
* 开仓逻辑：`programs/lp_handler/src/instructions/swap_and_deposit.rs`
* 加仓逻辑：`programs/lp_handler/src/instructions/increase_liquidity.rs`
* 减仓/领奖逻辑：`programs/lp_handler/src/instructions/decrease_liquidity.rs`
* 事件/错误/工具函数：`programs/lp_handler/src/state/`
* 调用方式示例：`tests/lp_deposit.test.ts`、`tests/lp_withdraw.test.ts`、`tests/lp_claim.test.ts`

---

## 1. 合约概览与定位

`lp_handler` 是一个 **Raydium CLMM（AmmV3）“组合交易/仓位操作”封装层**，把用户侧常见的三类动作做成稳定的链上入口：

* **开仓**：单边或双边资金 ->（必要时 swap）-> 新建仓位并加流动性
* **加仓**：向已有 Position 增加流动性 ->（必要时 swap）-> 处理剩余
* **减仓/领奖**：减仓或纯领奖 ->（可选先 swap 归一）-> **只对池子两币的 reward 抽成**

核心目标：

* **降低集成复杂度**：把 Raydium CLMM 的多次 CPI 串成单条指令；
* **内置关键约束**：tick 区间、mint 合法性、fee 收款白名单、Position NFT ATA 推导一致性、remaining accounts 基础校验；
* **可观测性**：对 swap、开仓、加仓、减仓/领奖提供事件。

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

## 3. 总体数据流（三条核心指令）

#### `swap_and_deposit`

1. User 调用 `swap_and_deposit(amount_0_in, amount_1_in, return_mint, tick_lower, tick_upper, quoted_mode, quoted_sqrt_price_x64, slippage_bps, swap_amount_in, swap_min_out, swap_input_is_token0)`
2. 合约读取 `pool_state`，校验本次执行时的价格模式是否仍与链下报价一致：
  + `quoted_mode` 必须与链上当前区间状态一致
  + `quoted_sqrt_price_x64` 作为价格锚点，用于校验当前价格没有跌破可接受下界
  + 不再允许链上覆盖/改写调用方给出的 swap 计划
3. 如需 swap：CPI 调用 Raydium `swap_v2`（tick arrays/bitmap 由 remaining accounts 提供）
4. CPI 调用 Raydium `open_position_with_token22_nft`，铸造 Position NFT（Token2022）并开仓
5. 处理“剩余”：可选将剩余归一到 `return_mint`；若 `min_out == 0` 则按规则处理（非 wSOL → 转 fee；wSOL → 留给用户并在末尾 close/unwrap 成 SOL）
6. 事件：`IncreaseLiquidityEvent`（包含 `return_amount_0/return_amount_1`）

#### `increase_liquidity`

1. User 调用 `increase_liquidity(amount_0_in, amount_1_in, return_mint, tick_lower, tick_upper, quoted_mode, quoted_sqrt_price_x64, slippage_bps, swap_amount_in, swap_min_out, swap_input_is_token0)`
2. 合约先校验传入的 `tick_lower/tick_upper` 必须与已有 `personal_position` 完全一致
3. 后续 zap、价格锚点和 cleanup swap 规则与 `swap_and_deposit` 保持一致

#### `decrease_liquidity`

1. User 调用 `decrease_liquidity(liquidity, mint_amount_0, mint_amount_1, swap_to_token_mint, quoted_sqrt_price_x64, slippage_bps, fee_percent, convert_to_target_mint)`
2. 合约校验 `fee_owner` 白名单与 fee ATA
3. CPI 调用 Raydium `decrease_liquidity_v2`（奖励相关 remaining accounts 在分隔符之后）
4. 通过“余额增量 delta - principal_expected”拆分 reward；仅对 reward 抽成
5. 若 `convert_to_target_mint=true`：可能再 CPI `swap_v2` 把 reward/principal 兑换到目标币种后再扣费
6. 事件：`DecreaseLiquidityEvent`

整体上，这三条指令共享几类基础约束：

* 安全层入口/出口快照
* `SecurityConfig` 中的 pool / fee owner 白名单
* zap 报价模式校验（quoted mode + quoted price anchor）
* Token2022 / ATA / fee vault 的一致性约束
---

## 4. 指令一： `swap_and_deposit`

### 4.1 入口与参数

入口： `programs/lp_handler/src/lib.rs` -> `instructions::swap_and_deposit`

参数语义：

* `amount_0_in: u64`：本次允许的 token0 最大投入量（最小单位）
* `amount_1_in: u64`：本次允许的 token1 最大投入量（最小单位）
* `return_mint: Option<Pubkey>`：可选；若提供则必须等于池子 token0 或 token1 的 mint，用于“把剩余尽量归一到某一边”
* `tick_lower_index/tick_upper_index: i32`：仓位 tick 区间，要求 `lower < upper`
* `quoted_mode: u8`：链下报价时记录的 zap 模式；链上执行时必须与当前区间状态一致
* `quoted_sqrt_price_x64: u128`：链下报价时记录的价格锚点；链上据此校验最低可接受价格
* `slippage_bps: u16`：滑点（bps），该指令中要求 `< 5000`
* `swap_amount_in: u64 / swap_min_out: u64 / swap_input_is_token0: bool`：
  + 作为链下 plan 输入（最多执行一次主 swap）
  + 链上不会再根据执行时价格自动覆盖该 plan，只会校验它是否与 `quoted_mode` 匹配

### 4.2 账户模型与关键约束

Accounts： `SwapAndDeposit<'info>`

关键约束点（非穷尽）：

* `raydium_clmm_program` 必须是 `raydium_amm_v3::ID`
* `pool_state.load()?.amm_config == amm_config.key()`
* `token_vault_0/1` 必须匹配 `pool_state.load()?.token_vault_0/1`
* `user_token0_account` 的 mint 必须是 `token_vault_0.mint`；`user_token1_account` 同理
* `tick_array_lower/tick_array_upper` 必须是当前 `pool_state` 与 lower/upper start index 对应的 Raydium tick array PDA
  + 对 `swap_and_deposit` 而言，不要求这两个 PDA 在进入本程序前就已经初始化为 `TickArrayState`
  + 合约会校验 PDA 地址是否正确，真正的初始化/owner 语义由 Raydium `open_position_with_token22_nft` CPI 处理
* **Position NFT（Token2022）ATA 地址校验**：
  + `position_nft_account` 必须等于 `ATA(position_nft_owner, position_nft_mint, TOKEN_2022_PROGRAM_ID)`
  + 注意：合约只校验“地址推导是否正确”，允许 ATA 尚未创建（由 CPI 创建）

### 4.3 执行步骤

1. 参数校验：tick 区间、`return_mint` 合法性、slippage 上限
2. 读池状态：`sqrt_price_x64`、`tick_spacing`，并校验 `quoted_mode + quoted_sqrt_price_x64`
3. 按调用方给出的 plan 执行主 swap（最多一次）；链上不会再自动覆盖方向/数量
4. 如需 swap：计算 `min_out` 并 CPI 调用 Raydium `swap_v2`
5. 计算 `amount_0_max/amount_1_max` 作为开仓上限（swap 后余额 + 输入预算）；流动性计算会先扣除 Token-2022 transfer fee 再估算
6. CPI 调用 Raydium `open_position_with_token22_nft`
7. 处理剩余并发事件：`swap_back_remaining_and_emit_increase_event`
  + `return_mint=None`：两边剩余都保留在用户 token account
  + `return_mint=Some(token0/token1)`：尽量把另一边剩余兑换为目标币种；若目标侧最小输出 `< MIN_USDC_SWAP_AMOUNT`：
    - 非 wSOL：该侧剩余转给 `fee_token*_account`
    - wSOL：不转 fee，留给用户，并在末尾 close/unwrap 成 SOL
8. 发 `IncreaseLiquidityEvent`（新增字段 `return_amount_0/return_amount_1`）

#### `return_amount_0/return_amount_1` 口径（case-by-case）

在 `swap_back_remaining_and_emit_increase_event` 中：

* `leftover_0 = amount_0_max - spent_0`
* `leftover_1 = amount_1_max - spent_1`
* `return_amount_0/return_amount_1` 是事件里记录的“最终留给用户的剩余”口径（留在用户的 `signer_token0/1_account` 里；若 mint 是 wSOL，后面会 close/unwrap 成 SOL）。

Case 1： `return_mint == None` （不要求把剩余统一换成某一边）

* `return_amount_0 = leftover_0`
* `return_amount_1 = leftover_1`

Case 2： `return_mint == token0` （希望把 token1 剩余换成 token0）

* **2.1** `leftover_1 == 0`
  + `return_amount_0 = leftover_0`
  + `return_amount_1 = 0`
* **2.2** `leftover_1 > 0` 且目标侧最小输出 `>= MIN_USDC_SWAP_AMOUNT`（执行 swap token1→token0）
  + `return_amount_0 = leftover_0 + out_from_swap`
  + `return_amount_1 = 0`
  + 如果 cleanup swap 执行失败：不回滚主流程，保留 `leftover_0/leftover_1` 在用户账户
* **2.3** `leftover_1 > 0` 且目标侧最小输出 `< MIN_USDC_SWAP_AMOUNT`（不 swap）
  + **2.3.a** token1 不是 wSOL：`leftover_1` 转给 fee
    - `return_amount_0 = leftover_0`
    - `return_amount_1 = 0`
  + **2.3.b** token1 是 wSOL：`leftover_1` 不转 fee，留给用户（后面 close→SOL）
    - `return_amount_0 = leftover_0`
    - `return_amount_1 = leftover_1`

Case 3： `return_mint == token1` （希望把 token0 剩余换成 token1）

* **3.1** `leftover_0 == 0`
  + `return_amount_1 = leftover_1`
  + `return_amount_0 = 0`
* **3.2** `leftover_0 > 0` 且目标侧最小输出 `>= MIN_USDC_SWAP_AMOUNT`（执行 swap token0→token1）
  + `return_amount_1 = leftover_1 + out_from_swap`
  + `return_amount_0 = 0`
  + 如果 cleanup swap 执行失败：不回滚主流程，保留 `leftover_0/leftover_1` 在用户账户
* **3.3** `leftover_0 > 0` 且目标侧最小输出 `< MIN_USDC_SWAP_AMOUNT`（不 swap）
  + **3.3.a** token0 不是 wSOL：`leftover_0` 转给 fee
    - `return_amount_1 = leftover_1`
    - `return_amount_0 = 0`
  + **3.3.b** token0 是 wSOL：`leftover_0` 不转 fee，留给用户（后面 close→SOL）
    - `return_amount_1 = leftover_1`
    - `return_amount_0 = leftover_0`

额外说明（wSOL & rent）：

* `close_account` 会把 token account 的 lamports 全部转走，因此 **rent 也会一起转走**。
* 事件里的 `return_amount_*` 记录的是 token `amount` 口径（不含 rent）；实际钱包收到的 SOL 会比该数值多一点点（包含 rent）。

### 4.4 remainingAccounts 规则

`swap_and_deposit` / `increase_liquidity` 的 `ctx.remaining_accounts` 现在采用 **四段分隔协议**：

* 结构：`[main_swap_remaining] + [lp_handler programId] + [action_remaining] + [lp_handler programId] + [cleanup_swap_remaining_input_token0] + [lp_handler programId] + [cleanup_swap_remaining_input_token1]`
* 第一段：主 swap 用的 bitmap/tick arrays
* 第二段：Raydium `open_position_with_token22_nft` 或 `increase_liquidity_v2` 所需 remaining accounts
* 第三段：cleanup swap 在“实际 leftover 输入为 token0”时使用的 bitmap/tick arrays
* 第四段：cleanup swap 在“实际 leftover 输入为 token1”时使用的 bitmap/tick arrays
* cleanup swap 属于 best-effort 步骤：
  + 主 swap 与开仓/加仓成功后，如 cleanup swap 因 CU、路径或状态偏差失败，不回滚主流程
  + 失败时两边 leftover 保留在用户账户

合约对 remaining 做了轻量校验：

* `swap_and_deposit` / `increase_liquidity` 入口会显式要求存在三个分隔符
* 两个 cleanup 候选段都不能为空
* 真正传给 Raydium `swap_v2` 的 remaining accounts 仍要求 `owner == raydium_clmm_program`

### 4.5 对照测试用例（调用约定）

参考： `tests/lp_deposit.test.ts`

* 用 Raydium SDK 计算主 swap 需要的 tick arrays，放在第一段
* 在第一段后插入一次 `program.programId` 分隔符
* 把 open position 所需 remaining accounts 放在第二段
* 再插入一次 `program.programId` 分隔符
* 把 “cleanup 输入 token0” 需要的 tick arrays 放在第三段
* 再插入一次 `program.programId` 分隔符
* 把 “cleanup 输入 token1” 需要的 tick arrays 放在第四段
* `positionNftMint` 用 `Keypair.generate()` 生成并作为签名者
* `positionNftAccount` 采用 Token2022 的 ATA 推导（`getATAAddress(user, mint, TOKEN_2022_PROGRAM_ID)`）
* 交易层通常会提高 compute limit/price

---

## 5. 指令二： `decrease_liquidity`

### 5.1 入口与参数

入口： `programs/lp_handler/src/lib.rs` -> `instructions::decrease_liquidity`

参数语义：

* `liquidity: u128`
  + `>0`：典型减仓（本金 + 奖励）
  + `=0`：典型领奖（只领奖励；principal 视为 0）
* `mint_amount_0/mint_amount_1: u64`：传给 Raydium `decrease_liquidity_v2` 的参数（测试里多为 0）
* `swap_to_token_mint: Pubkey`：目标 mint（仅在 `convert_to_target_mint=true` 时生效），必须是池子 token0 或 token1
* `quoted_sqrt_price_x64: u128`：链下报价时记录的价格锚点；链上据此做兑换路径的价格下界校验
* `slippage_bps: u16`：`<= 5000`
* `fee_percent: u16`：抽成比例（bps），`<= 10000`
* `convert_to_target_mint: bool`：是否把本次结算尽量统一兑换到 `swap_to_token_mint`

### 5.2 账户模型与关键约束

Accounts： `DecreaseLiquidity<'info>`

关键约束：

* `fee_owner` 必须命中 `security_config` PDA 中的 `fee_owners` 白名单（运行时校验）
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
  + `liquidity == 0`：principal 视为 0（领奖语义）
  + `liquidity > 0`：用当前价格与区间估算 `principal_expected_0/1`
* `reward_gross = delta - principal_expected`（使用 `saturating_sub`，避免出现负数）
* 抽成：`integrator_fee = reward_gross * fee_percent / 10000`

### 5.5 兑换路径 convert_to_target_mint=true（实现语义：兑换到目标币种）

当 `convert_to_target_mint=true` ：

* `swap_to_token_mint` 决定目标边（token0 或 token1）
* 兑换流程为“合并 swap + 近似拆分”：
  1. 将对侧 token 的本次增量（principal + reward）合并成一次 `swap_v2` 兑换为目标币种
  2. 用比例近似把 swap 输出拆分为 `reward_out_est / principal_out_est`
  3. 手续费仍然只按 reward 口径计提：`fee = (reward_direct + reward_out_est) * fee_percent`
* `DecreaseLiquidityEvent` 在该模式下会把 principal/reward/fee **集中体现在目标币种一侧**，另一侧置 0

当 `convert_to_target_mint=false` ：

* 不换币，对 token0/token1 各自 reward 分别抽成并分别 transfer 到 fee_token0/fee_token1 ATA

### 5.6 Dust 规则（关键）

当 `convert_to_target_mint=true` 且进入“纯领奖 + 对侧奖励需要兑换”的路径时，合约会先按目标侧价格锚点估算本次兑换的 `min_out`。

规则如下：

* 只有当 **目标侧估算输出** 小于 `MIN_USDC_SWAP_AMOUNT` 时，才视为 dust
* 这里比较的是“目标侧输出口径”，不是把任意 mint 的 raw amount 直接与 USDC 常量比较
* dust 场景下不会执行 swap，目的是避免兑换输出为 0 或过小导致无意义 swap / 失败
* dust 场景下，对应的 non-target reward 会 **整笔转到手续费地址**

注意：

* 这条规则是特定业务设计，不是普通 reward 抽成逻辑
* 非 dust 的正常路径下，仍然只对池子两币的 reward 按 `fee_percent` 抽成
* 额外激励代币不参与这条 dust-兑换规则

---

## 6. 事件（可观测性）

事件定义见 `programs/lp_handler/src/state/events.rs` ：

* `IncreaseLiquidityEvent`：加仓 amount0/amount1、tick 区间、position nft mint 等
  + `return_amount_0/return_amount_1`：两侧最终退回给用户的剩余（最小单位；wSOL 会在函数末尾 close/unwrap 成 SOL，实际到账会额外包含 rent）
* `DecreaseLiquidityEvent`：principal、reward、integrator_fee（按“兑换后口径”或“双币口径”输出）

---

## 7. 错误码与常见排查

错误码见 `programs/lp_handler/src/state/errors.rs` ：

* `InvalidRemainingAccounts`
  + swap_and_deposit / increase_liquidity：主 swap / cleanup swap 的 Raydium remaining 超过 32、owner 不正确、缺少四段协议分隔符，或任一 cleanup 候选段为空
  + decrease_liquidity：找不到分隔符或 swap_remaining 校验失败
* `InvalidPositionNftAccount`：Position NFT ATA 地址推导不匹配
* `InvalidDepositMint`：`return_mint`（若提供）/ `swap_to_token_mint` 不是池子 token0/token1
  + swap_and_deposit：`return_mint` 不合法（不是池子 token0/token1）时也会触发
* `InvalidFeeOwner`/`InvalidFeeTokenAccount`：fee 白名单或 fee ATA 校验失败
* `InvalidTickRange`：tick 区间非法
* `InvalidSlippage`/`InvalidFeePercent`：参数越界
* `NoBalanceChange`：仅在 `liquidity > 0` 且 token0/token1 两边增量都为 0 时触发；`liquidity == 0` 的 reward-only claim 不再因该条件回滚
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
* 协议设计上只对池子两币的 reward 抽成；Raydium 外部 farming / incentive reward 不参与 fee 计算。
* 外部 farming reward 的实际接收账户仍由 `remainingAccounts` 中提供的 reward token accounts 决定；合约当前不会再对这部分 reward 做二次归集或重路由。

---

## 9. 与测试用例的对应关系

* `tests/lp_deposit.test.ts`：存入（swap + open_position）与 swap remainingAccounts 构造示例
* `tests/lp_withdraw.test.ts`：减仓（含兑换 + close position）与 decrease/claim 路径的旧版三段式 remainingAccounts 组织示例
* `tests/lp_claim.test.ts`：领奖（`liquidity=0`）与 decrease/claim 路径的旧版三段式 remainingAccounts 组织示例

---

## 10.  交易示例

* withdraw : https://solscan.io/tx/4FP1XmPP16xFEUfnwhebY1GEskSzDbyQxNBCycpmUt6zPS1UVmdzkqB3P8J5C6BpgiXfYh2xz8KDSEG5AEdDcFRf
* deposit: https://solscan.io/tx/Tr5YioQQ5erjrvGBM87zmx5pmf6md3D8hiGdGcDXbPZye6rvSe891rBFJS38tCg3LGstgAtrWcF9dadiKDzrBys
* claim:  https://solscan.io/tx/4V91uuLWYLwho7M6a9qh6CBUDNUJaxCUyTG1mCLfV8MjU57QoeYSwVZEccQe4rgtgPbAgWwAMtfHKGoesmzn4Gc

 
