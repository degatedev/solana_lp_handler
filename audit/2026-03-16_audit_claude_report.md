# DeGate LP Handler 安全审计报告

**审计代码**: `dev` 分支, commit `b7c1a09`

**审计日期**: 2026-03-16
**审计执行**: Claude (Anthropic)
**审计范围**: `programs/lp_handler/src/` 全部文件
**前序审计**: DeGate 内部审计 (2026-02-06, 21项)、Adevar Labs 外部审计 (2026-03-13, 18项)

---

## 一、前序审计修复验证

### Adevar Labs 外部审计 (H01-E07)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| H01 | Lamport 注入 DoS SecurityConfig PDA | High | Fixed | `security/mod.rs:140-142` 改为入口快照/出口相对校验 `exit_lamports <= entry_lamports` |
| M01 | Token-2022 transfer fee 未纳入计算 | Medium | Fixed | `zap_common.rs:518-527` deposit 侧扣减； `decrease_liquidity.rs:277-288` decrease 侧也扣减 |
| M02 | Out-of-range auto-swap 覆盖用户 min_out | Medium | Fixed | 引入 `quoted_mode` + `quoted_sqrt_price_x64` ，链上不再自动覆盖 swap 计划 |
| M03 | Zap swap 无价格边界保护 | Medium | Fixed | `zap_common.rs:202-212` 使用 `derive_main_swap_price_limit` 基于 tick boundary |
| M04 | SecurityConfig init 可被抢跑 | Medium | Fixed | `build.rs` 编译期注入 `SECURITY_ADMIN` ， `security_config.rs:23-29` init 时校验 |
| M05 | Claim/convert 路径缺少报价约束 | Medium | Fixed | `decrease_liquidity.rs:452-456` 添加 `validate_price_floor_from_quote` |
| L01 | Cleanup swap 复用主 swap remaining | Low | Acknowledged | 已拆分为四段 remaining_accounts，cleanup 为 best-effort |
| L02 | Dust 判断用 USDC 常量比较任意 mint | Low | Fixed | `decrease_liquidity.rs:979-983` 改为基于目标侧估算输出判断 |
| L03 | Dust 分支没收 100% reward | Low | Acknowledged | 保留业务设计 |
| L04 | Fee token account 无 canonical ATA 约束 | Low | Fixed | `decrease_liquidity.rs:986-1018` 强制验证 canonical ATA |
| L05 | Farming reward 不计费 | Low | Acknowledged | 产品决策 |
| E01 | USDC_MIN 命名 | Enhancement | Fixed | 更名为 `USDC_MINT` |
| E02 | Token-2022 native mint 处理不一致 | Enhancement | Fixed | 统一 `is_native_sol_mint` helper |
| E03 | 未使用的死代码 | Enhancement | Fixed | 已删除 `collect_accounts_to_check` |
| E04 | Tick array 约束不一致 | Enhancement | Fixed | 统一校验， `swap_and_deposit` 允许未初始化 PDA |
| E05 | fee_owners 可为空 | Enhancement | Fixed | init/update 强制非空 |
| E06 | increase_liquidity tick 参数不校验 | Enhancement | Fixed | `increase_liquidity.rs:250-265` 校验与仓位一致 |
| E07 | convert_to_usdc 命名 | Enhancement | Fixed | 更名为 `convert_to_target_mint` |

### DeGate 内部审计 (LPH-001 ~ LPH-021)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| LPH-001 | fee_owner 未验证 | Critical | Fixed | `security/mod.rs:334-338` 白名单校验 |
| LPH-002 | recipient 未链上验证 | Critical | Acknowledged | 依赖前端 + LPH-007 |
| LPH-003 | 入口不拒绝已有 delegate | High | Fixed | `security/mod.rs:374-388` 入口拒绝 |
| LPH-004 | saturating_sub 掩盖异常 | High | Fixed | `decrease_liquidity.rs:317-340` 显式分支 + WARN |
| LPH-005 | exit check 未复核账户身份 | High | Fixed | `security/mod.rs:80,572-581` 存 (idx, Pubkey) |
| LPH-006 | Position NFT 校验不一致 | High | Fixed | `decrease_liquidity.rs:162-168` 强类型约束 |
| LPH-007 | recipient 未验证在账户列表 | High | Fixed | `security/mod.rs:295-296` |
| LPH-008 | 奖励拆分整数除法截断 | Medium | Fixed | `decrease_liquidity.rs:513-528` 四舍五入 |
| LPH-009 | 最小输出固定减 100 | Medium | Fixed | `utils.rs:58-109` 重构为纯数学 |
| LPH-010 | 费用计算复合精度损失 | Medium | Fixed | `utils.rs:99-100` 合并单次除法 |
| LPH-011 | init 无管理员门禁 | Medium | Acknowledged | 部署流程缓解 + M04 修复 |
| LPH-012 | authority 不可轮换 | Medium | Acknowledged | immutable 合约模式 |
| LPH-013 | Token-2022 extension 未检查 | Medium | Acknowledged | pool whitelist 运营审核 |
| LPH-014 | close 阻断所有操作 | Medium | Acknowledged | 正常迁移流程 |
| LPH-015 | 配置变更不发事件 | Low | Fixed | `events.rs:79-114` 三个事件 |
| LPH-016 | unwrap() 可能 panic | Low | Fixed | `utils.rs:144-146` try_into + map_err |
| LPH-017 | 未使用 checked_mul | Low | Fixed | `security/mod.rs:180-183` |
| LPH-018 | unwrap_or(0) 掩盖错误 | Low | Fixed | `decrease_liquidity.rs:713-720` WARN 日志 |
| LPH-019 | fee_percent 可为零 | High | Acknowledged | 灵活费率设计 |
| LPH-020 | 手续费向下取整可拆分利用 | Low | Acknowledged | 经济不可行 |
| LPH-021 | release 未开溢出检查 | Low | Fixed | `Cargo.toml:8` overflow-checks = true |

**结论：两份审计报告共 39 项，全部已正确处理（Fixed 或 Acknowledged with justification）。**

---

## 二、新发现

### [NEW-01] validate_price_floor_from_quote 仅做单向价格下限检查 — LOW

**位置**: `zap_common.rs:305-320`

**描述**: 只检查 `current_price >= quoted_price * (1 - slippage)` ，无上界校验。价格被拉高时，卖出方向的 swap 可能在不利价格执行。

**缓解因素**:
* zap 路径主 swap 有 `swap_min_out` 由链下锚定
* zap 路径有 `sqrt_price_limit_x64` 基于 tick boundary 限价
* decrease 路径有 `swap_other_amount_threshold` 基于 `quoted_sqrt_price_x64` 锚定

**建议**: 可考虑添加上界检查形成双向价格窗口，但当前保护已充分，优先级低。

---

### [NEW-02] `calculate_epoch_fee` 静默回退 0 — LOW — 已修复

**位置**: `utils.rs:13-26`

**原始问题**: `transfer_fee_config.calculate_epoch_fee(epoch, pre_fee_amount).unwrap_or(0)` 计算失败时静默返回 0，可能导致 Token-2022 token 的 transfer fee 被低估。

**修复方案**: 提取为独立函数 `calculate_transfer_fee_from_config` ，失败时返回 `MathOverflow` 错误而非静默回退。同时新增单元测试 `transfer_fee_config_errors_when_epoch_fee_cannot_be_derived` 覆盖异常路径。

**修复验证**: 已确认修复正确且完整。

---

### [NEW-03] remaining_accounts 不做全量白名单扫描 — LOW

**位置**: `security/mod.rs:441-474`

**描述**: 安全层对 `remaining_accounts` 仅扫描 `authority == signer` 的 token accounts，不对其他账户做白名单校验。

**缓解因素**:
* 信任模型前提：第三方 DAPP 不作恶
* swap remaining accounts 通过 `swap_v2_accounts` 强制 owner 为 Raydium 程序
* 这是有意识的设计权衡（降低内存峰值）

**建议**: 无需修改，在安全架构文档中明确标注该设计决策。

---

### [NEW-04] program_owned_lamports 快照不覆盖 remaining_accounts — LOW

**位置**: `security/mod.rs:367-369`

**描述**: 仅对 `ctx.accounts` 中 `owner == crate::ID` 的账户做 lamports 快照。

**缓解因素**: SecurityConfig PDA 是唯一的 program-owned 账户，已在 `ctx.accounts` 中。 `remaining_accounts` 实际不包含本程序拥有的账户。

**建议**: 无需修改，实际风险为零。

---

### [NEW-05] cleanup swap 使用 `sqrt_price_limit_x64 = 0` — LOW

**位置**: `zap_common.rs:657-663` , `zap_common.rs:748-754`

**描述**: Cleanup swap 无价格边界。

**缓解因素**:
* 金额通常很小（CPI 后的 leftover）
* 有 `min_out` 保护
* 失败不回滚主流程（best-effort）
* 对应 Adevar L01，已 Acknowledged

**建议**: 维持当前设计，加强链下监控。

---

## 三、代码质量评估

### 正面

* **安全架构扎实**: Entry/Exit 安全模型提供了强力边界，业务逻辑无法绕过
* **算术安全**: 全面使用 `checked_*` / `U256` + `overflow-checks = true`
* **审计修复质量高**: 代码干净、注释带审计编号、可追溯
* **防御纵深**: Anchor 约束 → 安全层入口 → 安全层出口 → 业务逻辑校验
* **测试覆盖**: 每个模块有针对性单元测试

### 关注点

* **CU 消耗**: 安全层全量账户遍历在账户较多时需注意 CU 上限
* **wSOL 处理**: unwrap/fee 支付路径复杂度较高，是边界 case 温床
* **事件估算值**: decrease 事件中的 `settled` 数据基于比例拆分，链下使用需注意精度

---

## 四、总结

| 维度 | 评级 |
|------|------|
| 安全架构 | 优秀 |
| 前序审计修复完成度 | 优秀 (39/39 处理完成) |
| 代码质量 | 良好 |
| 新发现风险 | 低 |

本次审计发现 5 项 LOW 级别问题，其中 NEW-02 已修复（ `calculate_epoch_fee` 静默回退 0 改为显式错误传播），其余 4 项均为设计权衡，无需代码修改。

未发现 Critical、High 或 Medium 级别的新问题。

**合约已达到生产环境安全标准。**
