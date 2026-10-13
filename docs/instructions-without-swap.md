# 不含 Swap 的开仓 / 加仓 — 需求文档

## 1. 背景

现有 LP 合约的开仓和加仓默认会带一个"链上 swap 配平"步骤：用户传入两边 token，合约先自动 swap 把比例调整到当前价位再注入流动性。

但部分场景用户已经在链下完成了最优比例计算，**不希望合约再做任何 swap**：

- 链下报价系统已经按当前池价精确算好两边投入额，链上多一次 swap 反而带来滑点损失
- 部分集成方希望路径只走 LP 注入，避免 swap 路径的 MEV 风险
- 简化指令逻辑，减少计算单元（CU）消耗

为此提供两条"不含 swap"指令：

- `open_position_without_swap`：新建仓位
- `increase_liquidity_without_swap`：向已有仓位追加流动性

---

## 2. 用户故事

### 2.1 普通钱包用户首次建仓

> "我已经在前端选好了价格区间和两边的投入额，希望直接按这个比例建仓，不要被合约二次 swap。多余的零头退回到我自己的钱包。"

### 2.2 普通钱包用户加仓

> "我已经持有一个 CLMM 头寸，想再追加一笔流动性。区间已经定好了，我只想按当前定下的比例追加进去。"

### 2.3 硬件钱包 / 代付场景

> "我用的是硬件冷钱包做接收方（recipient），但实际签名付费的是另一个热钱包（signer）。我希望零头能选择性退到 signer 或 recipient，不要被强制只能退给 recipient。"

### 2.4 集成方对接

> "我们是接入方，希望合约接口尽量贴近 Raydium 原生 CLMM 的语义，但保留白名单、抽成位的占位（虽然这两条指令不抽成），方便后续切换。"

---

## 3. 功能需求

### 3.1 通用需求

1. **不进行任何 swap**。两边投入额完全由调用方决定，链上不重新分配比例。
2. **不收取手续费**。不做任何抽成，所有未投入到池子的零头全额退回。
3. **滑点保护**：调用方传入 quoted liquidity，链上重新基于当前价位计算 liquidity，差距超过滑点容忍度则失败。
4. **零头自动退回**：投入完成后，剩余 token 自动退回。退回逻辑详见 §5.2。
5. **wSOL 自动 unwrap**：若任一侧是 wSOL，对应零头会被自动 unwrap 成原生 SOL 退回。详见 §5.2。
6. **NFT 归属 recipient**：`open_position_without_swap` mint 的头寸 NFT，归属人是 recipient（不是 signer）。详见 §5.3。
7. **池子白名单校验**：仅允许在合约白名单内的 pool 执行。
8. **签名者与接收方分离**：支持 signer 和 recipient 是不同钱包的场景（硬件钱包 / 代付）。
9. **Refund 账户允许双 authority**：refund 账户的 owner 可以是 signer 也可以是 recipient，由调用方按业务选择，不允许第三方账户。

### 3.2 `open_position_without_swap` 专属需求

1. 在指定 pool 内为 recipient **新建一个 CLMM 仓位**，mint 出 Token-2022 格式的头寸 NFT。
2. 调用方需指定价格区间（tick_lower / tick_upper）和两边的最大投入额。
3. 价格区间必须有效（lower < upper），区间外（单边）也允许。
4. NFT 归属人是 recipient，不是 signer。

### 3.3 `increase_liquidity_without_swap` 专属需求

1. 向**已存在**的 CLMM 仓位追加流动性，仓位由调用方传入的 NFT 标识。
2. **不允许**调用方指定 tick 区间——区间从链上仓位状态直读，避免传错。
3. 调用方必须实际持有该仓位的 NFT（NFT account amount = 1，authority = signer）。

---

## 4. 输入输出

### 4.1 调用方需要提供

**两条指令共同**：

- 价格区间内两边各愿意投入的最大额度
- quoted liquidity（链下基于当前价位算好的预期值）
- 滑点容忍度（基点）
- signer / recipient 钱包
- 两边 token 的来源账户（owner = signer）
- 两边 token 的退款账户（owner = signer 或 recipient）
- pool 与对应金库账户

`**open_position_without_swap` 额外需要**：

- 价格区间 tick_lower / tick_upper
- 一对新生成的 NFT mint keypair（随交易签名）
- NFT 接收账户（一般是 recipient 的 ATA）
- 价格区间对应的 tick array 账户（允许未初始化）

`**increase_liquidity_without_swap` 额外需要**：

- 已有的仓位状态账户
- 持有该仓位 NFT 的 token account
- 价格区间对应的 tick array 账户（必须已初始化）

### 4.2 调用方会得到

1. 链上事件，包含：
  - pool / NFT mint / token mints
  - 实际投入的两边数量（principal_0 / principal_1）
  - 实际生成的 liquidity
  - 退回的零头数量（return_amount_0 / return_amount_1）
  - 价格区间
2. 失败时收到明确的错误码（参数错误 / 余额不足 / 滑点超限 / 白名单未通过 等）。

---

## 5. 业务规则

### 5.1 校验规则


| 规则                                                        | 行为  |
| --------------------------------------------------------- | --- |
| 两边投入额都是 0                                                 | 拒绝  |
| 滑点容忍度 ≥ 50%                                               | 拒绝  |
| signer 余额不够支付投入额                                          | 拒绝  |
| refund 账户不属于 signer 也不属于 recipient                        | 拒绝  |
| pool 不在白名单内                                               | 拒绝  |
| `open_position`：tick_lower ≥ tick_upper                   | 拒绝  |
| `open_position`：tick_array 地址与 pool/tick 不匹配              | 拒绝  |
| `increase_liquidity`：调用方未持有该仓位 NFT                        | 拒绝  |
| 链上 computed liquidity 与 quoted liquidity 之差 / quoted > 滑点 | 拒绝  |


### 5.2 零头退回规则

#### 5.2.1 基本概念

- `**amount_in`**：调用方传入的两边最大投入额（`amount_0_in` / `amount_1_in`）。
- `**spent**`：CPI 后通过对比 signer token 账户余额变化算出的实际花费 = `balance_before - balance_after`。
- `**leftover**` = `amount_in - spent`：该侧的零头。
- **来源账户**：`signer_token_X`（owner = signer）。所有 CPI 资金都从这里出。
- **退款账户**：`refund_token_X`，owner 必须是 signer 或 recipient（运行时校验，不允许第三方）。

#### 5.2.2 退款触发条件（按顺序判断）

对 token0 和 token1 各执行一次以下流程：

**情况 A — 该侧 mint 是 wSOL**

不在普通退款阶段做任何转账。理由：wSOL 收尾逻辑会在指令尾部统一处理：

- 强制 close 掉 `signer_token_X`（必须是 wSOL 账户，无论是不是 ATA）
- close 后 `signer_token_X` 中的 wSOL 全部 unwrap 成原生 SOL
- SOL 退回目标地址：
  - 若 `signer == recipient`：退给 signer
  - 若 `signer != recipient`：退给 recipient

> ⚠️ 这意味着 wSOL 侧的零头**不会**走 `refund_token_X`，refund 账户在 wSOL 侧的设置会被忽略。调用方在传 wSOL 侧的 refund 账户时仍需通过 mint/authority 校验，但实际不会被使用。

**情况 B — 该侧 mint 不是 wSOL，且 `refund_token_X == signer_token_X`**

不做任何转账。零头本来就留在 signer 自己的账户里。

**情况 C — 该侧 mint 不是 wSOL，且 `refund_token_X != signer_token_X`**

调用 SPL Token 转账：从 `signer_token_X` 转 `leftover` 数量到 `refund_token_X`。

- 由 signer 作为 authority 签名（已经是交易 signer，无需额外签名）。
- 使用 SPL Token 或 Token-2022 程序，按 mint 类型自动选择。
- 若 mint 带 transfer fee 扩展，链上转账会自动扣除转账费，refund 账户实际收到 `leftover - transfer_fee`。

#### 5.2.3 调用方根据需求选择 refund 账户


| 业务诉求            | refund 账户应该是                                   |
| --------------- | ---------------------------------------------- |
| 普通钱包，零头留在自己钱包里  | signer 自己的 ATA（建议直接传 signer_token_X 同地址，省一次转账） |
| 普通钱包，零头单独走另一个账户 | 同 signer 持有的另一个 token account                  |
| 硬件钱包代付，零头退给冷钱包  | recipient 的 ATA                                |
| 硬件钱包代付，零头留在热钱包  | signer 的 ATA                                   |


**禁止**：refund 账户的 owner 既不是 signer 也不是 recipient（拒绝，错误 `InvalidRefundAuthority`）。

#### 5.2.4 退款金额示例

假设 `amount_0_in = 1000`，`amount_1_in = 2000`，CPI 后实际花掉 `spent_0 = 970`、`spent_1 = 1980`：

- `leftover_0 = 30`、`leftover_1 = 20`
- token0 不是 wSOL、refund_token0 与 signer_token0 不同 → 转 30 个 token0 到 refund_token0
- token1 是 wSOL → 跳过普通退款；指令尾部 close signer_token1，连本带息 unwrap 成 SOL 退给目标钱包（signer 或 recipient）

#### 5.2.5 事件中的退款字段

最终事件 `LpHandlerIncreaseLiquidityEvent` 会带：

- `principal_0` / `principal_1`：实际花费（即 `spent`）
- `return_amount_0` / `return_amount_1`：退回的零头（即 `leftover`，**未扣除可能的 Token-2022 转账费**）

下游索引器据此可推算资金流向。

### 5.3 NFT 接收逻辑（仅适用于 `open_position_without_swap`）

#### 5.3.1 基本概念

- **NFT mint**：每次 open_position 都新生成一个 Token-2022 mint，代表这一笔仓位的所有权。
- **NFT 接收账户（`position_nft_account`）**：用来持有这枚 NFT 的 token account，amount 始终为 1。
- **NFT 归属人（持有该 token account 的 authority）**：决定后续谁能动这个仓位（加仓 / 减仓 / 领奖）。

#### 5.3.2 NFT 归属规则

- **归属人固定为 recipient，不是 signer**。即便由 signer 付费 mint，NFT 也直接进入 recipient 的账户。
- 这与含 swap 的开仓指令保持一致，目的是让"付费方"和"持仓方"可以分离（硬件钱包 / 代付）。
- 这条规则由 Raydium CPI 内部强制执行，本合约不再额外校验。

#### 5.3.3 NFT 接收账户的传入方式

调用方需在指令账户里传入 `position_nft_account`，链上不会自动派生：


| 场景                      | 推荐做法                                                                                                                |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------- |
| 标准做法（推荐）                | 传 recipient 在 Token-2022 程序下的 ATA：`getAssociatedTokenAddressSync(nft_mint, recipient, true, TOKEN_2022_PROGRAM_ID)` |
| ATA 不存在                 | 由 Raydium CPI 内部使用 ATA 程序自动创建（已在账户列表里包含 `associated_token_program`），调用方无需在外部预先创建                                    |
| 非 ATA 自定义 token account | 不推荐。本合约不校验地址派生，但 Raydium CPI 会校验 `mint == nft_mint`、`owner == recipient`，错则失败                                       |


#### 5.3.4 mint keypair 的处理

- `position_nft_mint` 是一个**新的 keypair**，不是 PDA。
- 调用方在客户端临时生成一对 keypair，**该 keypair 的私钥必须随交易共同签名**（`signers: [walletSigner, nftMintKeypair]`）。
- 签完即可丢弃私钥，因为：
  - mint 后该账户的 mint authority 会被 Raydium 内部转移 / 冻结，私钥不再具备意义
  - 后续操作（加仓 / 减仓 / 领奖）只看 NFT account 的 authority，不用 mint 本身签名
- 如果调用方忘记把 mint keypair 加进 signers，交易会因缺签名而失败。

#### 5.3.5 加仓 / 减仓时的对接

后续对该仓位的 `increase_liquidity_without_swap` / 含 swap 的加仓 / 减仓指令：

- 由 NFT 当前持有人（即 recipient 或 recipient 之后转手的新 owner）作为 signer。
- `position_nft_account` 必须是该 owner 持有这枚 NFT 的 token account（amount = 1，authority = signer）。
- 这两条不含 swap 的指令在 increase 场景下，链上通过 anchor 约束 `position_nft_account.amount == 1` 且 `authority == signer` 强制保证调用者真正持有该仓位 NFT。

#### 5.3.6 NFT 转手

- NFT 是普通 Token-2022 标准 NFT，可通过 SPL Token 转账自由转手。
- 转手后新 owner 即获得对该仓位的所有控制权。
- 本合约不限制 NFT 转手，但 transfer 后调用 increase / decrease 时，调用者必须以新 owner 身份签名。

### 5.4 不收费

虽然账户列表里保留 `fee_owner` 占位（用于安全层校验，与其他指令统一），但本指令不抽成、不向 fee_owner 转任何 token。

---

## 6. 非功能需求

1. **白名单**：复用现有 SecurityConfig PDA 的 pool 白名单。
2. **指令对齐**：账户布局、remaining_accounts 分隔符协议与现有含 swap 的指令保持一致，方便客户端共用拼装逻辑。
3. **不破坏 Raydium 原生约束**：tick array 派生规则、NFT 归属规则、PersonalPosition PDA 规则均保留 Raydium 原始语义。
4. **事件复用**：复用 `LpHandlerIncreaseLiquidityEvent`，不新增事件类型，方便下游索引器统一消费。
5. **错误码可读**：每种校验失败都有专属错误码，前端可针对不同错误展示不同提示。

---

## 7. 范围之外（不在本需求内）

- ❌ 链上 swap：本指令明确不做 swap，需要 swap 的场景请使用 `swap_and_deposit` / `increase_liquidity`。
- ❌ 抽成：本指令不收手续费。
- ❌ 自动建 ATA：refund / NFT 接收账户必须由调用方在调用前确保已存在。
- ❌ 多池路由：单笔指令只针对一个 pool。

---

## 8. 验收标准

1. 普通钱包用户能成功新建仓位并加仓，零头正确退回到自己钱包。
2. 硬件钱包场景下，refund 账户可以是 signer 或 recipient，二者均可成功；第三方账户被拒绝。
3. 当链下 quoted liquidity 与链上 computed liquidity 偏差超过滑点容忍度时，交易回滚，资金不损失。
4. 任一侧为 wSOL 时，零头以 SOL 形式退回，账户中不残留 wSOL。
5. 非白名单 pool 调用直接被安全层拒绝。
6. 事件输出的 principal / return / liquidity 字段与链上实际状态一致，可被下游索引器消费。
7. 不含 swap 路径下，单笔交易消耗的计算单元应明显低于含 swap 的对应指令。

