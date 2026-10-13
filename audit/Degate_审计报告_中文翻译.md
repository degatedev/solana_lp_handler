# Solana LP Handler 审计报告

**审计机构：** Adevar Labs
**委托方：** Degate
**日期：** 2026年3月20日
**版本：** 修复后审查版

---

## 1. 执行摘要

### 关于 Degate

*Degate 的 Solana LP Handler 是基于 Raydium CLMM 流动性管理流程的封装程序。它支持头寸创建、流动性增减、代币兑换以达到所需存款比例、奖励和手续费收取，以及为集成商手续费路由和运营安全提供的协议级安全控制。*

### 审计总结

| 风险等级 | 数量 | 已修复 | 已确认(不修复) |
|---------|------|-------|---------------|
| 严重(Critical) | 0 | 0 | 0 |
| 高危(High) | 1 | 1 | 0 |
| 中危(Medium) | 5 | 5 | 0 |
| 低危(Low) | 4 | 1 | 3 |

改进建议：7 项

### 整体安全状况

Degate 团队展示了对 Raydium CLMM 原语的深入理解，初步审计采用了安全优先的思维方式。然而，围绕兑换计划、滑点执行、奖励处理和安全配置方面的发现表明，对抗性边界情况尚未被完全建模。代码库功能完整，但在上线前需要更强的防御性验证。

### 修复后审查总结

Degate 团队及时解决了所有高危和中危问题，并展示了对安全影响的深入理解。

### 修复前审查总结

审计识别了1个与 SecurityConfig 中协议级拒绝服务风险相关的高危问题、5个影响转账费用核算/zap执行/滑点保护/初始化加固/价格边界执行的中危问题，以及4个围绕粉尘处理/费用路由/奖励核算的低危问题。此外还识别了7项改进命名清晰度/验证一致性/可维护性的增强建议。

### 上线建议

**I. 上线前必须完成：**
- 解决 SecurityConfig 中的1个高危可用性问题，包括 lamport 注入拒绝服务条件和安全配置流程所需的初始化加固
- 修复5个中危问题，涉及转账费用核算、兑换滑点执行、价格边界执行和领取路径三明治攻击抵御
- 对 swap_and_deposit、increase_liquidity 和 decrease_liquidity 进行端到端重新测试

**II. 强烈建议：**
- 解决围绕费用路由、粉尘处理、奖励核算和代币账户规范化的4个低危问题
- 实施7项改进建议
- 为清理兑换、奖励收取、费用所有者账户选择以及边界条件添加回归测试

**III. 上线后监控：**
- 监控失败交易和链上日志以快速检测回归
- 跟踪费用收取余额、奖励代币流向和目标账户模式
- 对涉及 SecurityConfig PDA 的异常事件进行告警

### 审计范围

- **代码仓库：** https://github.com/degatedev/solana_lp_handler
- **修复前提交哈希：** 17ea3eac2fb57ea79f69ffb9aad01627aa530134
- **修复后提交哈希：** c09ed65b8b1ed572c0d743d69a15064134cc8fc1
- **范围内文件/模块：** programs/*

---

## 2. 假设与注意事项

### 中心化风险
**I. 管理员权限：** 控制安全配置，需依赖管理员对协议支持的池和受信任的费用管理者地址进行尽职调查。

### 信任假设
**II. 集成商费用仅在前端执行：** 已知集成商费用可通过直接调用链上程序绕过。前端被信任来配置合理的费用值。

### 运营假设
**III. 仅支持 USDC 配对池：** 该协议仅支持其中一个代币为 USDC 的 Raydium 池。

---

## 3. 严重等级定义

每个问题基于**影响度**和**可能性**两个维度分配严重等级。

*注：改进建议代表非阻塞性改进，不构成直接资产风险，但实施后可提高可靠性和用户体验。*

### 影响度
- **高影响：** 可导致用户/协议资金完全损失、核心功能中断或永久丧失关键组件控制
- **中影响：** 可导致重大中断或部分资金损失，但非完全损害
- **低影响：** 后果较小或可忽略，可能影响边界情况或略微降低性能

### 可能性
- **高可能性：** 可被轻易利用，无需特权访问，资金需求低
- **中可能性：** 需要中等努力发现和利用，可能需要大量资金投入
- **低可能性：** 技术上通常不可行或需要高度专业化条件

### 严重等级矩阵
- **严重(Critical)：** 高影响 + 高可能性
- **高危(High)：** 高影响+中可能性，或中影响+高可能性
- **中危(Medium)：** 中等影响和/或中等可发现性
- **低危(Low)：** 最小影响或不太可能被利用

---

## 4. 审计发现

---

### H01：向 SecurityConfig PDA 注入 Lamport 将导致所有协议操作拒绝服务

| 属性 | 值 |
|------|-----|
| 影响度 | 中-高 |
| 可能性 | 高 |
| 严重等级 | **高危** |
| 状态 | **已解决** |

**位置：** security/mod.rs:L508-L516

**相关代码：**
```rust
for ai in accounts.iter() {
    if ai.owner == &crate::ID {
        let min = rent.minimum_balance(ai.data_len());
        require!(
            ai.lamports() <= min,
            LpDepositError::SecurityProgramLamportsLeaked
        );
    }
}
```

**描述：** 任何外部参与者都可以通过向 SecurityConfig PDA 发送1个 lamport 来阻止所有业务指令（swap_and_deposit、increase_liquidity、decrease_liquidity）。exit_check 验证所有程序拥有账户的 lamport 数量不超过 rent.minimum_balance(data_len)，这是绝对值比较而非快照比较。

在 Solana 上任何人都可以向任何地址转 SOL（包括PDA）。仅发送1个 lamport 就会导致 exit_check 失败。攻击无需权限，成本极低（约0.000005 SOL），且每次管理员修复后可重复。虽无资金风险（交易原子回滚），但所有协议操作将被无限期阻止。

**概念验证：**
1. 攻击者推导 SecurityConfig PDA
2. PDA 以恰好 rent.minimum_balance(1520) = 11,337,600 lamports 初始化
3. 攻击者通过 system_program::transfer 发送1 lamport，无需权限
4. PDA 现有 11,337,601 lamports
5. 用户调用 swap_and_deposit()：入口检查通过 → 业务逻辑成功 → exit_check 时 ai.lamports() = 11,337,601 > min → **失败回滚**
6. 管理员可通过原子调用 close + init 恢复，但攻击者可重复

**建议：** 将 exit_check 中的 lamport 检查从绝对比较改为快照比较：在 entry_check_and_snapshot 捕获进入时余额，退出时验证未超过进入值。

**开发者回应：** *"lamport 检查已从绝对租金限制改为进入快照/退出相对比较，防止外部 lamport 转入 PDA 造成的全局 DoS。"*

---

### M01：lp_handler 代币金额计算未考虑 Token2022 转账费用

| 属性 | 值 |
|------|-----|
| 影响度 | 中 |
| 可能性 | 中 |
| 严重等级 | **中危** |
| 状态 | **已解决** |

**位置：** zap_common.rs:L360-L366

**描述：** lp_handler 使用原始（毛额）金额进行所有代币计算，未扣除 Token2022 转账费用。在存款路径导致流动性计算偏差，在减少路径导致余额对账失败。

**建议：**
- 存款路径：参照 Raydium 的方法，计算流动性前先扣除转账费用
- 减少路径：比较前从预期本金中扣除转账费用

**开发者回应：** *"存款/zap 端流动性计算已支持 Token-2022 转账费用，减少路径已对账扣费后余额变化。"*

---

### M02：prepare_zap_plan_and_swap_if_needed 超出范围自动兑换覆盖用户 min_out，使三明治攻击成为可能

| 属性 | 值 |
|------|-----|
| 影响度 | 中 |
| 可能性 | 中 |
| 严重等级 | **中危** |
| 状态 | **已解决** |

**位置：** zap_common.rs:L192-L221

**描述：** 当头寸超出范围时，程序自动配置兑换并用基于当前现货价格计算的 exec_swap_min_out 覆盖用户提供的 swap_min_out。由于 calc_min_amount_out 中滑点参数应用于已被操纵的价格上，攻击者可进行三明治攻击。

**PoC 要点：**
- 用户设 swap_min_out=19000（5%滑点保护），当前价格2.0
- 攻击者先卖120,000 token0，操纵价格
- 程序覆盖 min_out 为15,146（基于被操纵价格）
- 受害者实际获得15,802 < 19,000（用户预期），但 > 15,146（链上阈值），交易成功
- 攻击者获利约2,035 token0

仅影响超出范围的自动兑换分支，范围内路径仍使用调用者的计划。

**建议：** 停止用现货价格推导值替换用户认证的执行下限，保留用户控制的滑点保护。

**开发者回应：** *"引入了 quoted_mode 和 quoted_sqrt_price_x64 验证，防止超出范围自动兑换覆盖用户执行保护。"*

---

### M03：超出范围 zap 流程不执行价格边界，导致存款失败或双倍兑换费用

| 属性 | 值 |
|------|-----|
| 影响度 | 中 |
| 可能性 | 中 |
| 严重等级 | **中危** |
| 状态 | **已解决** |

**位置：** zap_common.rs:L191-L222

**描述：** 超出范围存款的 zap 流程兑换用户全部对立代币余额，但不强制兑换不能将价格推入头寸 tick 边界。当 sqrt_price_limit_x64=0 时，Raydium 默认无边界，兑换直到全部输入消耗。

**根本原因：** 超出范围覆盖在兑换前做决策（全部兑换），但从未验证兑换后前提条件是否仍成立。

**后果：**
- **路径A（回滚）：** 大额存款价格影响超过 slippage_bps，交易回滚，无损失但存款被阻止
- **路径B（粉尘剩余）：** 影响在 slippage_bps 内，通过检查但 get_liquidity_from_amounts 返回极小值，几乎全部存款成为剩余被回兑，用户支付双倍兑换费获得无价值头寸

**PoC（路径B）：** SOL/USDC池，用户存50 SOL 开 [$148,$150] 范围头寸，仅需13.6 SOL到达边界但全部50 SOL被兑换越过边界。最终用户损失$37.46兑换费，头寸近零流动性。

**建议：** 在 Raydium swap CPI 中使用 sqrt_price_limit_x64 强制 tick 边界：
```rust
let sqrt_price_limit = if exec_swap_input_is_token0 {
    get_sqrt_price_at_tick(tick_upper_index)?  // 卖token0，价格下降，在上界停止
} else {
    get_sqrt_price_at_tick(tick_lower_index)?  // 卖token1，价格上升，在下界停止
};
```
并在兑换后验证头寸范围状态未改变。

**开发者回应：** *"主 zap 兑换现在同时强制 quoted_mode、quoted_sqrt_price_x64、swap_min_out 和 sqrt_price_limit_x64 tick 边界检查。"*

---

### M04：无权限 SecurityConfig 初始化可被前置运行

| 属性 | 值 |
|------|-----|
| 影响度 | 高 |
| 可能性 | 低 |
| 严重等级 | **中危** |
| 状态 | **已解决** |

**位置：** security_config.rs:L81

**描述：** init_security_config 接受任何签名者作为 authority。第一个初始化 PDA 的账户将永久控制池白名单和费用所有者白名单。每次 close_security_config 后重新初始化窗口会重新打开。

由于可在同一交易中原子性关闭和重开 PDA，重新初始化风险实际较低。但 Solana 部署存在可见性延迟（slot N 部署，slot N+1 生效），无法在部署同一交易中初始化。

**建议：** 将 init_security_config 限制为受信任的治理或管理员权限。

**开发者回应：** *"init_security_config 仍需受限引导管理员，但不再硬编码。部署脚本通过 SECURITY_ADMIN 环境变量注入部署者钱包。"*

---

### M05：decrease_liquidity 领取路径(liquidity=0)因使用链上现货价格计算最小输出而无法防御三明治攻击

| 属性 | 值 |
|------|-----|
| 影响度 | 中 |
| 可能性 | 中 |
| 严重等级 | **中危** |
| 状态 | **已解决** |

**位置：** decrease_liquidity.rs:L384-L395

**描述：** 当 decrease_liquidity 以 convert_to_usdc=true 调用时，转换兑换的最小输出阈值完全使用当前链上价格计算。由于阈值和兑换在同一交易对同一池状态执行，阈值总是通过。slippage_bps 不能防御交易前的外部价格操纵。

对于仅领取调用（liquidity=0, convert_to_usdc=true），Raydium 跳过 mint_amount_0/1 检查，仅剩 slippage_bps 但从被操纵价格计算，无实际保护。

**PoC：** 池 SOL/USDC 公平价格$100，用户累积25 SOL费用。攻击者前置运行将价格压到$97，用户以$97执行获得$2412而非$2500。**用户损失约$88**。

**建议：** 添加可选的用户提供的 convert_min_out 参数，让前端从市场价格离链计算最小输出。

**开发者回应：** *"报价约束和价格保护已添加到领取/转换路径，与存款路径保持一致。"*

---

### L01：重用 swap_remaining 账户可导致 swap_and_deposit / increase_liquidity 回滚

| 属性 | 值 |
|------|-----|
| 影响度 | 中 |
| 可能性 | 低 |
| 严重等级 | **低危** |
| 状态 | **已确认(不修复)** |

**位置：** swap_and_deposit.rs:L244

**描述：** 两个入口点保存单个 swap_remaining 切片并在清理兑换中重用。这不安全，因为 Raydium 的 remaining_accounts 是路径相关的 CLMM 状态。主兑换改变池状态后，清理兑换可能需要不同的 tick 数组。

**建议：** 将清理兑换建模为独立的 Raydium 兑换，使用单独的账户切片。

**开发者回应：** *"仅在主兑换恰好停在 tick 数组边界的罕见情况下才可能失败。交易回滚，用户资金不受影响，重试即可。概率极低，无资金风险。"*

---

### L02：粉尘奖励分支没收100%非目标奖励并从费用核算中省略

| 属性 | 值 |
|------|-----|
| 影响度 | 低 |
| 可能性 | 低 |
| 严重等级 | **低危** |
| 状态 | **已确认(不修复)** |

**位置：** decrease_liquidity.rs:L408-L412

**描述：** convert_to_usdc 流程中，粉尘处理分支在 principal_other_in==0 且兑换金额低于 MIN_USDC_SWAP_AMOUNT 时触发，将全部奖励（而非按 fee_percent 比例）作为费用转给 fee_owner。后续费用计算和事件也不反映此金额。

**建议：** 仅收取 reward_other_in * fee_percent / 10_000，返还余额，并在事件中记录。

**开发者回应：** *"这是有意的产品设计：当目标代币估计输出太小时，奖励直接转给费用地址。残余风险已知并接受。"*

---

### L03：无约束的费用代币账户将导致费用碎片化

| 属性 | 值 |
|------|-----|
| 影响度 | 低 |
| 可能性 | 低 |
| 严重等级 | **低危** |
| 状态 | **已解决** |

**位置：** decrease_liquidity.rs:L82-L93

**描述：** 账户模型不强制 fee_token0/1_account 是 fee_owner 的规范 ATA。任何正确 mint 且 authority 为白名单 fee_owner 的代币账户都能通过，与 README 文档矛盾。

**建议：** 在链上推导预期 ATA 并拒绝不匹配。

**开发者回应：** *"fee_token0_account 和 fee_token1_account 现在要求是 fee_owner 拥有的对应 mint 的规范 ATA。"*

---

### L04：decrease_liquidity 缺乏对外部挖矿奖励的显式处理

| 属性 | 值 |
|------|-----|
| 影响度 | 低-中 |
| 可能性 | 中 |
| 严重等级 | **低危** |
| 状态 | **已确认(不修复)** |

**位置：** decrease_liquidity.rs:L289-L292

**描述：** 指令对交易费奖励收取集成商费用，但不处理 Raydium 池可分发的最多3个外部挖矿奖励。导致：
1. **无费用收取：** 挖矿奖励从未被跟踪/收费，可能超过交易费 APR
2. **不受控路由：** 奖励去向取决于 remaining_accounts 中的账户，后端可将奖励路由给 fee_owner（获取100%）

资金不会被锁定——用户可直接通过 Raydium 领取挖矿奖励。

**建议：** 在 CPI 后逻辑中显式处理挖矿奖励，快照余额变化并按 fee_percent 收费。

**开发者回应：** *"仅奖励领取已更新不再因 NoBalanceChange 回滚，但我们有意不对外部挖矿奖励收取协议费。这是有意的产品决策。"*

---

## 5. 改进建议

### E01：将 USDC_MIN 重命名为 USDC_MINT
**位置：** consts.rs:L33
常量存储 USDC mint 地址但读起来像"最小值"，易与 MIN_USDC_SWAP_AMOUNT 混淆。

### E02：Token 2022 原生 mint 处理不一致
**位置：** zap_common.rs:L826-L835
关闭代币账户时检查 Token 2022 原生 mint，但其他地方仅检查 SPL Token 原生 mint。建议引入统一的辅助函数。

### E03：移除未使用的函数 collect_accounts_to_check
**位置：** security/mod.rs:L254-L266
该函数在仓库中未被调用，是死代码且具有误导性。

### E04：跨指令统一 tick 数组验证
**位置：** swap_and_deposit.rs:L97-L103
swap_and_deposit 使用 UncheckedAccount 无池约束，而 increase_liquidity/decrease_liquidity 使用 AccountLoader 带约束。建议统一。

### E05：强制非空 fee_owners 白名单
**位置：** security_config.rs:L42-L50
init/update 允许 fee_owners=[] 但运行时安全层要求非空。空列表会导致所有指令以 InvalidFeeOwner 回滚。建议在写入时强制非空。

### E06：increase_liquidity 中验证 tick 参数与现有头寸匹配
**位置：** increase_liquidity.rs:L183-L184
接受调用者 tick 参数但不验证是否匹配存储的头寸 tick。不匹配可触发错误的自动兑换，导致双倍费用。

**开发者回应：** *"increase_liquidity 现在强制 tick_lower_index/tick_upper_index 匹配现有 personal_position。"*

### E07：将 convert_to_usdc 重命名为 convert_to_target_mint
**位置：** decrease_liquidity.rs:L27
参数名具误导性，实际启用转换为 swap_to_token_mint 传入的任何 mint，非仅 USDC。

---

## 关于我们

### 关于 Adevar Labs
Adevar Labs 是专注于 web3 审计的精品区块链安全公司。团队背景涵盖 Bitdefender、Asymmetric Research、Quantstamp 等，拥有 Solana、Ethereum、Polkadot、Cosmos 等生态系统的安全经验。已完成超过100次审计，保护超过5亿美元链上价值。

### 审计方法论
1. **程序上下文和架构分析：** 深入检查文档、功能和账户设计
2. **威胁建模：** 针对 Solana 执行环境的威胁建模
3. **深入手动安全审查：** 逐行检查，关注 Solana 常见漏洞
4. **详细修复审查和验证：** 逐一验证修复有效性

### 保密声明
本报告受保密条款约束，未经 Adevar Labs 授权不得披露、复制或分发。

### 法律免责声明
本报告按"原样"提供，不构成金融/投资/法律建议。区块链技术仍在开发中，存在未知风险。使用本报告需自行承担风险。
