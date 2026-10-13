# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

这是一个 Solana CLMM（Concentrated Liquidity Market Maker）LP 存取款管理合约，对 Raydium AMM V3 CLMM 的复合操作进行封装。Program ID：`GE63ymrB8fZUYd9oCKpda7ZQ6qdqc9rqq86Vs2jbRwoU`（Mainnet）。

## 常用命令

### 构建

```bash
# 必须先设置 SECURITY_ADMIN 环境变量（已在 .env 中）
export SECURITY_ADMIN=<admin_pubkey>
anchor build
```

### 测试

```bash
# 运行所有 Jest 集成测试
yarn test

# 运行单个测试文件
yarn test lp_deposit.test.ts
yarn test lp_withdraw.test.ts
yarn test lp_claim.test.ts

# Rust 单元测试
cargo test --package lp_handler --lib
```

### 部署与运维

```bash
# 交互式部署 CLI（包含安全边界校验）
yarn cli

# 更新安全边界摘要基准值（修改 security/mod.rs 或 lib.rs 后必须执行）
yarn update-security-boundary

# 代码格式化
yarn lint:fix
```

## 架构概述

### 安全边界（Security Boundary）

这是整个合约最关键的设计。`lib.rs` 中的 `secure_entrypoint!` 宏强制每条指令遵循"入口检查 → 业务逻辑 → 出口检查"的三段式结构，由 `security/mod.rs` 实现。

**固定不变的文件**（修改需要更新 `scripts/security_boundary.sha256`）：
- `programs/lp_handler/src/lib.rs`
- `programs/lp_handler/src/security/mod.rs`

安全检查包括：pool 白名单（`SecurityConfig` PDA）、fee_owner 白名单、入口/出口 token 权限一致性、SOL 不泄漏、账户集合一致性。

### 六条指令

| 指令 | 说明 |
|------|------|
| `swap_and_deposit` | 开仓（可选 swap 后建新 Position） |
| `increase_liquidity` | 加仓（可选 swap 后增加流动性） |
| `decrease_liquidity` | 减仓或纯领奖（`liquidity=0` 时为纯领奖） |
| `open_position_without_swap` | 不含 swap 的开仓 |
| `increase_liquidity_without_swap` | 不含 swap 的加仓 |
| `init/update/close_security_config` | 安全配置 PDA 管理 |

### remainingAccounts 分隔协议

所有含 swap 的指令使用固定分隔符协议，必须严格遵守：

```
[swap_remaining_accounts] + [lp_handler_program_id] + [action_remaining_accounts]
```

分隔符是 lp_handler 自身的 programId，客户端拼接时必须包含。

### Principal vs Reward 拆分（decrease_liquidity 核心）

减仓时仅对 reward 抽成，不对本金抽成。拆分逻辑：
1. 调用 Raydium `decrease_liquidity_v2` 后，通过余额增量计算 delta
2. 根据当前 tick/price 估算本金期望值（`calculate_principal_amounts_for_liquidity`）
3. `reward_gross = delta - principal_expected`（saturating_sub）
4. `fee = reward_gross * fee_percent / 10000`

### Zap 报价模式

`quoted_mode` 参数决定链下报价逻辑（0=单边 token0，1=单边 token1，2=双边）。`quoted_sqrt_price_x64` 用于执行时价格合理性校验。测试中的 `tests/utils.ts` 提供 `solveZapTwoSidedCLMM` 计算双侧最优 swap 量。

### Dust 规则

减仓 `convert_to_target_mint=true` 时：若目标侧估算输出 < `MIN_USDC_SWAP_AMOUNT`（1000），则不执行 swap，对应的 reward 整笔转给 fee_owner。

## 关键约束

- `slippage_bps` 最大值为 5000（50%）
- `fee_percent` 最大值为 10000（100%）
- SecurityConfig 中 `allowed_pools` 最多 40 个，`allowed_fee_owners` 最多 6 个
- Position NFT 使用 Token2022 程序
- Rust 工具链版本固定为 `1.89.0`（见 `rust-toolchain.toml`）

## 测试配置

测试运行在 Mainnet（真实 RPC），使用 `tests/help.ts` 中配置的 SOL-USDC 池（`3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv`）。运行测试前需要在 `.env` 中配置有效的 `ANCHOR_WALLET` 和 `ANCHOR_PROVIDER_URL`，且钱包需持有足够的测试资金。

`build.rs` 在编译时将 `SECURITY_ADMIN` 环境变量注入到合约二进制文件中，因此每次构建前必须设置该变量。

## 编码准则

### 1. 编码前思考

在实现前明确说明假设；有多种解读时列出选项，不默默选择；有更简单方案时主动说明；遇到不清楚的地方直接问，不猜测。

### 2. 简洁优先

只写解决问题所需的最少代码。不加未被要求的功能、抽象、"灵活性"或不可能发生场景的错误处理。如果能用 50 行写完，就不写 200 行。

### 3. 手术式修改

只改必须改的。不"顺手"优化无关代码、注释或格式。匹配现有风格。发现无关的死代码时说出来，不擅自删除。自己的改动产生了孤儿导入/变量时，清理掉。每一行改动都应能直接追溯到用户的请求。

### 4. 目标驱动执行

把任务转化为可验证的目标。多步骤任务先给出简短计划并明确每步的验证方式，然后执行到可验证为止再报告结果。
