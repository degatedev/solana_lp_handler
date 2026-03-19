# DeGate LP Handler 安全审计报告

**审计代码**: `dev` 分支, 基于 commit `fc96be4` 的最新工作区代码
**审计日期**: 2026-03-19 (第三轮更新)
**审计执行**: Claude (Anthropic)
**审计范围**: `programs/lp_handler/src/` 全部文件
**前序审计**:
- DeGate 内部审计 (2026-02-06, 21 项)
- Adevar Labs 外部审计 before-fix-review (2026-03-13, 18 项)
- Adevar Labs 补充 fix review (2026-03-19, 4 项)
- Claude 第一轮复审 (2026-03-16, 5 项新发现)
- Codex 复审 (2026-03-19)

---

## 一、审计前提

本报告基于以下业务前提进行风险判断：

1. **仅 USDC 配对池**: 协议当前仅支持一侧为 USDC 的池子（如 USDC/SOL），dust 阈值逻辑依赖此前提。
2. **wSOL 一次性账户**: 用户侧 wSOL 账户为业务流程中按需创建、使用后关闭的临时账户。native mint 路径统一结算为原生 SOL，关闭 token account 属预期语义。
3. **白名单运营维护**: `security_config` 的 pool / fee_owner 白名单由运营侧正确维护。
4. **第三方调用约定**: 安全模型假设第三方 DAPP 不主动构造恶意交易。

若上述前提发生变化，需重新评估相关路径的风险结论。

---

## 二、前序审计修复验证

### Adevar Labs 外部审计 (H01-E07)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| H01 | Lamport 注入 DoS SecurityConfig PDA | High | Fixed | `security/mod.rs` 改为入口快照/出口相对校验 `exit_lamports <= entry_lamports` |
| M01 | Token-2022 transfer fee 未纳入计算 | Medium | Fixed | deposit 侧 `zap_common.rs` 扣减 transfer fee 后计算 liquidity；decrease 侧 `decrease_liquidity.rs` 对 principal 也做了 transfer fee 扣减 |
| M02 | Out-of-range auto-swap 覆盖用户 min_out | Medium | Fixed | 引入 `quoted_mode` + `quoted_sqrt_price_x64`，链上不再自动覆盖 swap 计划 |
| M03 | Zap swap 无价格边界保护 | Medium | Fixed | `derive_main_swap_price_limit` 基于 `QuotedZapMode + direction` 同时覆盖 in-range 和 out-of-range 边界 |
| M04 | SecurityConfig init 可被抢跑 | Medium | Fixed | `build.rs` 编译期注入 `SECURITY_ADMIN`，`security_config.rs` init 时校验 |
| M05 | Claim/convert 路径缺少报价约束 | Medium | Fixed | `decrease_liquidity.rs` 添加 `validate_price_from_quote` 双向价格偏差校验 |
| L01 | Cleanup swap 复用主 swap remaining | Low | Acknowledged | 已拆分为四段 remaining_accounts（main_swap / action / cleanup_token0 / cleanup_token1），cleanup 为 best-effort |
| L02 | Dust 判断用 USDC 常量比较任意 mint | Low | Fixed | 统一以 USDC 计价金额与 `MIN_USDC_SWAP_AMOUNT` 比较；target 为 USDC 用输出估算值，target 非 USDC 用输入值（即 USDC 侧） |
| L03 | Dust 分支没收 100% reward | Low | Acknowledged | 保留业务设计 |
| L04 | Fee token account 无 canonical ATA 约束 | Low | Fixed | `decrease_liquidity.rs` 强制验证 fee_token0/1_account 为 canonical ATA |
| L05 | Farming reward 不计费 | Low | Acknowledged | 产品决策 |
| E01 | USDC_MIN 命名 | Enhancement | Fixed | 更名为 `USDC_MINT` |
| E02 | Token-2022 native mint 处理不一致 | Enhancement | Fixed | 统一 `is_native_sol_mint` helper，同时识别 SPL Token 和 Token-2022 的 native mint |
| E03 | 未使用的死代码 | Enhancement | Fixed | 已删除 `collect_accounts_to_check` |
| E04 | Tick array 约束不一致 | Enhancement | Fixed | `swap_and_deposit` 通过 PDA 地址推导校验 tick array，兼容未初始化账户 |
| E05 | fee_owners 可为空 | Enhancement | Fixed | init/update 均强制 `!fee_owners.is_empty()` |
| E06 | increase_liquidity tick 参数不校验 | Enhancement | Fixed | `increase_liquidity.rs` 校验传入 tick 必须与已有仓位一致 |
| E07 | convert_to_usdc 命名 | Enhancement | Fixed | 更名为 `convert_to_target_mint` |

### Adevar Labs 补充 fix review (2026-03-19)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| M1 | `calculate_transfer_fee_from_config` 对 `MAX_FEE_BASIS_POINTS` 特判冗余 | Medium | Fixed | 移除特殊分支，完全委托 `calculate_epoch_fee`（内部已处理 `min(raw_fee, maximum_fee)`） |
| M2 | `validate_price_floor_from_quote` 仅保护价格下跌方向 | Medium | Fixed | 重命名为 `validate_price_from_quote`，改为双向绝对偏差检查 `|current - quoted| <= quoted * slippage` |
| M3 | `derive_main_swap_price_limit` 缺少 out-of-range → in-range 保护 | Medium | Fixed | 新增 `mode` 参数，out-of-range 时反转 tick 边界方向；同时修复审计建议中 `unreachable!()` 的 panic 风险（改为 `err!()` + 延迟计算） |
| L2 | Dust 阈值将非 USDC 金额与 USDC 常量比较（两侧覆盖不全） | Low | Fixed | target 为 USDC 用 `swap_other_amount_threshold`，target 非 USDC 用 `total_other_in`（即 USDC 输入金额） |

### DeGate 内部审计 (LPH-001 ~ LPH-021)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| LPH-001 | fee_owner 未验证 | Critical | Fixed | `security/mod.rs` 白名单校验 |
| LPH-002 | recipient 未链上验证 | Critical | Acknowledged | 依赖前端 + LPH-007 账户存在性校验 |
| LPH-003 | 入口不拒绝已有 delegate | High | Fixed | 入口阶段拒绝 signer token accounts 上已有的 delegate / close_authority |
| LPH-004 | saturating_sub 掩盖异常 | High | Fixed | 显式分支 + WARN 日志 |
| LPH-005 | exit check 未复核账户身份 | High | Fixed | 存 `(idx, Pubkey)` 并在出口复核 |
| LPH-006 | Position NFT 校验不一致 | High | Fixed | 强类型约束 |
| LPH-007 | recipient 未验证在账户列表 | High | Fixed | `security/mod.rs` 校验 recipient 存在于账户列表 |
| LPH-008 | 奖励拆分整数除法截断 | Medium | Fixed | 四舍五入到最近整数 |
| LPH-009 | 最小输出固定减 100 | Medium | Fixed | 重构为纯数学公式 |
| LPH-010 | 费用计算复合精度损失 | Medium | Fixed | 合并为单次整体除法 |
| LPH-011 | init 无管理员门禁 | Medium | Acknowledged | 已由 M04 SECURITY_ADMIN 方案覆盖 |
| LPH-012 | authority 不可轮换 | Medium | Acknowledged | immutable 合约模式 |
| LPH-013 | Token-2022 extension 未检查 | Medium | Acknowledged | pool whitelist 运营审核 |
| LPH-014 | close 阻断所有操作 | Medium | Acknowledged | 正常迁移流程 |
| LPH-015 | 配置变更不发事件 | Low | Fixed | init/update/close 均发事件 |
| LPH-016 | unwrap() 可能 panic | Low | Fixed | `try_into()` + `map_err` |
| LPH-017 | 未使用 checked_mul | Low | Fixed | 防御性 `checked_*` |
| LPH-018 | unwrap_or(0) 掩盖错误 | Low | Fixed | WARN 日志 + 显式处理 |
| LPH-019 | fee_percent 可为零 | High | Acknowledged | 灵活费率设计 |
| LPH-020 | 手续费向下取整可拆分利用 | Low | Acknowledged | 经济不可行 |
| LPH-021 | release 未开溢出检查 | Low | Fixed | `Cargo.toml` overflow-checks = true |

**结论：三份审计报告 + 一次补充 fix review 共 43 项，全部已正确处理（Fixed 或 Acknowledged with justification）。**

---

## 三、Claude 自主发现

### [NEW-01] `calculate_epoch_fee` 静默回退 0 — LOW — 已修复

**位置**: `utils.rs:12-22`

**原始问题**: `calculate_epoch_fee().unwrap_or(0)` 计算失败时静默返回 0，可能导致 Token-2022 transfer fee 被低估。

**修复**: 提取为独立函数 `calculate_transfer_fee_from_config`，失败时返回 `MathOverflow` 错误。后续在 Adevar M1 fix review 中进一步简化，移除了 `MAX_FEE_BASIS_POINTS` 特殊分支。

**当前状态**: 已修复并通过测试验证。

---

### [NEW-02] remaining_accounts 不做全量白名单扫描 — LOW — 设计决策

**位置**: `security/mod.rs` entry_check_and_snapshot

**描述**: 安全层对 `remaining_accounts` 仅扫描 `authority == signer` 的 token accounts，不对其他账户做白名单校验。

**缓解因素**:
* 信任模型前提：第三方 DAPP 不作恶
* swap remaining accounts 通过 `swap_v2_accounts` 强制 owner 为 Raydium 程序
* 有意识的设计权衡（降低 SBF 堆内存峰值）

**建议**: 无需修改，在安全架构文档中明确标注。

---

### [NEW-03] program_owned_lamports 快照不覆盖 remaining_accounts — LOW — 无实际风险

**位置**: `security/mod.rs` entry_check_and_snapshot

**描述**: 仅对 `ctx.accounts` 中 `owner == crate::ID` 的账户做 lamports 快照。

**缓解因素**: SecurityConfig PDA 是唯一的 program-owned 账户，已在 `ctx.accounts` 中。`remaining_accounts` 不包含本程序拥有的账户。

**建议**: 无需修改。

---

### [NEW-04] cleanup swap 使用 `sqrt_price_limit_x64 = 0` — LOW — 设计决策

**位置**: `zap_common.rs` swap_back_remaining_and_emit_increase_event

**描述**: Cleanup swap 无价格边界。

**缓解因素**:
* 金额通常很小（CPI 后的 leftover）
* 有 `min_out` 保护
* 失败不回滚主流程（best-effort，catch Err 后保留 leftover）
* 对应 Adevar L01，已 Acknowledged

**建议**: 维持当前设计，加强链下监控。

---

### [NEW-05] 审计建议 `_ => unreachable!()` 存在 panic 风险 — LOW — 已修复

**位置**: `zap_common.rs:205-224` `derive_main_swap_price_limit`

**描述**: Adevar M3 建议使用 `_ => unreachable!()` 作为兜底分支。但当 `swap_amount_in == 0` 时，mode 校验不约束 `swap_input_is_token0`，`_` 分支实际可达，会导致 panic。

**修复**:
1. 兜底分支使用 `err!(LpDepositError::InvalidDepositAmount)` 代替 `unreachable!()`
2. 将 `derive_main_swap_price_limit` 调用移入 `if swap_amount_in > 0 { ... }` 内部，不需要时不计算

**当前状态**: 已修复，4 个新增测试覆盖所有合法 `(direction, mode)` 组合。

---

## 四、代码质量评估

### 正面

* **安全架构扎实**: 统一的 Entry/Exit 安全包装（`secure_entrypoint!` 宏）让业务逻辑无法绕过账户校验
* **算术安全**: 全面使用 `checked_*` / `U256` + `overflow-checks = true`，无 `unwrap()` 或 `as` 截断
* **审计修复质量高**: 代码注释带审计编号（如 `// LPH-004`、`// M3 修复`），可追溯
* **防御纵深**: Anchor 约束 → 安全层入口（白名单/快照）→ 业务逻辑校验 → 安全层出口（权限对账）
* **测试覆盖**: 33 个单元测试，每个安全关键函数有独立测试用例
* **修复迭代质量**: 在审计建议基础上发现并修复了额外问题（如 `unreachable!()` panic 风险）

### 关注点

* **CU 消耗**: 安全层全量账户遍历在账户较多时需注意 CU 上限
* **wSOL 处理路径复杂**: unwrap/fee 支付/结算涉及多个分支，是边界 case 温床
* **事件数据带估算性质**: decrease 事件中的 `settled` 字段基于比例拆分，链下使用需注意精度
* **USDC 配对池假设**: L2 dust 逻辑的正确性依赖此前提，扩展到非 USDC 池需要重新设计

---

## 五、测试验证记录

```bash
SECURITY_ADMIN=11111111111111111111111111111111 cargo test -p lp_handler --lib
```

```
running 33 tests
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

测试覆盖包括：
- 安全层 lamport 检查（快照相对比较）
- 分隔符数量校验（单/多分隔符协议）
- 价格偏差双向校验
- 主 swap price limit 四种模式
- Cleanup swap 账户选择
- Tick array PDA 地址校验
- Tick 参数与仓位一致性
- Dust 阈值判断
- Fee token account canonical ATA 校验
- Security admin 权限校验
- Transfer fee 异常处理

---

## 六、总结

| 维度 | 评级 |
|------|------|
| 安全架构 | 优秀 |
| 前序审计修复完成度 | 优秀 (43/43 处理完成) |
| 代码质量 | 良好 |
| 新发现风险 | 低 (5 项 LOW，2 项已修复，3 项为设计决策) |

**本次审计覆盖 3 份外部/内部审计报告 + 1 次补充 fix review + 自主代码审查，共 48 项。全部已正确处理。**

未发现 Critical、High 或 Medium 级别的新问题。

**合约在既定业务前提下已达到生产环境安全标准。**
