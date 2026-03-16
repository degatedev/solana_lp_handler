# DeGate LP Handler 安全复审报告

**审计代码**: 当前工作区代码  
**审计日期**: 2026-03-16  
**审计执行**: codex
**审计范围**: `programs/lp_handler/src/` 全部文件  
**审计方法**: 静态代码复审 + 既有审计结论交叉验证 + 单元测试验证  
**参考资料**: `audit/2026-02-06_fix.md` , `audit/2026-03-16_fix_decisions_CN.md` , `audit/2026-03-16_audit_report.md`

---

## 一、审计前提

本报告基于以下业务前提进行风险判断：

1. 用户侧的 wSOL 账户不是长期持有账户，而是业务流程中按需创建、使用后关闭的一次性账户。
2. 合约将 native mint 路径统一结算为原生 SOL，关闭对应 token account 属于预期语义，不视为异常副作用。
3. `security_config` 的 pool / fee_owner 白名单由运营侧正确维护。

若上述前提未来发生变化，则需要重新评估 native mint / wSOL 相关路径的风险结论。

---

## 二、前序审计修复验证

### Adevar Labs 外部审计 (H01-E07)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| H01 | Lamport 注入 DoS SecurityConfig PDA | High | Fixed | `security/mod.rs` 已改为入口快照 / 出口相对校验，不再依赖绝对 rent 上限 |
| M01 | Token-2022 transfer fee 未纳入计算 | Medium | Fixed | `zap_common.rs` 与 `decrease_liquidity.rs` 均已按 transfer fee 口径扣减后计算 |
| M02 | Out-of-range auto-swap 覆盖用户 min_out | Medium | Fixed | 已引入 `quoted_mode` 与 `quoted_sqrt_price_x64` ，链上不再自动重写 swap plan |
| M03 | Zap swap 无价格边界保护 | Medium | Fixed | 主 swap 已增加 `sqrt_price_limit_x64` ，按 tick boundary 限价 |
| M04 | SecurityConfig init 可被抢跑 | Medium | Fixed | `build.rs` 注入 `SECURITY_ADMIN` ， `init_security_config` 执行管理员校验 |
| M05 | Claim/convert 路径缺少报价约束 | Medium | Fixed | `decrease_liquidity.rs` 已增加 `validate_price_floor_from_quote` |
| L01 | Cleanup swap 复用主 swap remaining | Low | Acknowledged | 已拆分四段 `remaining_accounts` ，cleanup 改为 best-effort |
| L02 | Dust 判断用 USDC 常量比较任意 mint | Low | Fixed | 已改为基于目标侧估算输出判断 dust |
| L03 | Dust 分支没收 100% reward | Low | Acknowledged | 保留既有产品语义 |
| L04 | Fee token account 无 canonical ATA 约束 | Low | Fixed | 已强制 `fee_token0_account` / `fee_token1_account` 为 canonical ATA |
| L05 | Farming reward 不计费 | Low | Acknowledged | 产品决策，不视为代码漏洞 |
| E01 | USDC_MIN 命名 | Enhancement | Fixed | 已更名为 `USDC_MINT` |
| E02 | Token-2022 native mint 处理不一致 | Enhancement | Fixed | 已统一 `is_native_sol_mint` helper |
| E03 | 未使用的死代码 | Enhancement | Fixed | 已删除相关死代码 |
| E04 | Tick array 约束不一致 | Enhancement | Fixed | 已统一校验逻辑， `swap_and_deposit` 兼容未初始化 PDA |
| E05 | fee_owners 可为空 | Enhancement | Fixed | init/update 均已强制非空 |
| E06 | increase_liquidity tick 参数不校验 | Enhancement | Fixed | 已校验传入 tick 必须与仓位一致 |
| E07 | convert_to_usdc 命名 | Enhancement | Fixed | 已更名为 `convert_to_target_mint` |

### DeGate 内部审计 (LPH-001 ~ LPH-021)

| 编号 | 问题 | 严重程度 | 处理 | 验证结果 |
|------|------|---------|------|---------|
| LPH-001 | fee_owner 未验证 | Critical | Fixed | `security/mod.rs` 已强制校验 fee_owner 白名单 |
| LPH-002 | recipient 未链上验证 | Critical | Acknowledged | 当前依赖账户列表存在性与调用约定，不单独记为新问题 |
| LPH-003 | 入口不拒绝已有 delegate | High | Fixed | signer token account 在入口阶段已拒绝已有 delegate / close authority |
| LPH-004 | saturating_sub 掩盖异常 | High | Fixed | 已改为显式判断并记录 WARN |
| LPH-005 | exit check 未复核账户身份 | High | Fixed | 已记录 `(idx, Pubkey)` 并在出口复核 |
| LPH-006 | Position NFT 校验不一致 | High | Fixed | decrease 路径已统一 Position NFT 约束 |
| LPH-007 | recipient 未验证在账户列表 | High | Fixed | 已检查 recipient 必须存在于账户列表 |
| LPH-008 | 奖励拆分整数除法截断 | Medium | Fixed | 已改为四舍五入到最近整数 |
| LPH-009 | 最小输出固定减 100 | Medium | Fixed | 已改为统一数学计算逻辑 |
| LPH-010 | 费用计算复合精度损失 | Medium | Fixed | 已合并为单次整体除法 |
| LPH-011 | init 无管理员门禁 | Medium | Acknowledged | 已由 `SECURITY_ADMIN` 编译期注入方案缓解 |
| LPH-012 | authority 不可轮换 | Medium | Acknowledged | immutable 合约模式下接受该设计 |
| LPH-013 | Token-2022 extension 未检查 | Medium | Acknowledged | 依赖 pool whitelist 运营审核 |
| LPH-014 | close 阻断所有操作 | Medium | Acknowledged | 属于版本迁移流程的一部分 |
| LPH-015 | 配置变更不发事件 | Low | Fixed | init / update / close 均已发事件 |
| LPH-016 | unwrap() 可能 panic | Low | Fixed | 已改为显式错误传播 |
| LPH-017 | 未使用 checked_mul | Low | Fixed | 已补充防御性 checked 计算 |
| LPH-018 | unwrap_or(0) 掩盖错误 | Low | Fixed | decrease 路径已替换为 WARN + 显式处理 |
| LPH-019 | fee_percent 可控 | High | Acknowledged | 业务上允许灵活费率 |
| LPH-020 | 手续费向下取整可拆分利用 | Low | Acknowledged | 经济上不可行 |
| LPH-021 | release 未开溢出检查 | Low | Fixed | 已启用 `overflow-checks = true` |

**结论**: 前序审计中列出的核心安全问题已基本按预期落地，关键修复点均可在当前代码中验证到。

---

## 三、本轮复审结果

本轮复审未发现新的 `Critical`、`High`、`Medium` 或 `Low` 级有效漏洞。

上一版复审里记录的唯一新增问题 `NEW-01` 已完成修复：

* `transfer_fee_config.calculate_epoch_fee` 在异常情况下不再静默回落为 `0`
* 当前实现已改为 fail-closed，返回 `LpDepositError::MathOverflow`
* 已补充针对异常 fee 配置的单元测试，验证该分支会显式报错

当前实现位置：

* [utils.rs](/Users/hanyukai/Desktop/code/demo/solana-contract/lp_deposit/programs/lp_handler/src/state/utils.rs#L13)
* [utils.rs](/Users/hanyukai/Desktop/code/demo/solana-contract/lp_deposit/programs/lp_handler/src/state/utils.rs#L169)

修复后的核心逻辑：

```rust
transfer_fee_config
    .calculate_epoch_fee(epoch, pre_fee_amount)
    .ok_or(LpDepositError::MathOverflow.into())
```

---

## 四、已确认的设计语义

以下项目在本轮复审中被确认是业务设计，而非漏洞：

1. **wSOL / native mint 账户关闭语义**
`zap_common.rs` 与 `decrease_liquidity.rs` 在 native mint 路径会 close 对应 token account，将余额还原成原生 SOL。根据当前业务说明，这就是预期行为。

2. **Cleanup swap best-effort**
   cleanup swap 失败时不回滚主流程，剩余资产留在用户账户。这是可接受的可用性与复杂度权衡。

3. **fee_percent 可由调用侧控制**
   当前属于产品设计允许的灵活费率模型，不单独记为安全漏洞。

---

## 五、代码质量评估

### 正面

* **安全层结构清晰**: 统一的 entry / exit 安全包装让主业务逻辑很难绕过账户校验。
* **修复可追踪**: 旧审计问题大多能在当前代码中直接定位到修复点。
* **算术安全性较好**: 大量使用 `checked_*`、`U256` 和显式错误传播。
* **账户约束较完整**: pool whitelist、fee owner whitelist、canonical ATA、tick range 等核心边界已明显加固。

### 关注点

* **remaining_accounts 仍依赖调用约定**: 当前不是对所有 `remaining_accounts` 做全量白名单扫描，而是只做分段和关键账户校验，这是有意识的资源权衡。
* **native mint 路径较复杂**: 虽然按当前业务语义可接受，但该路径仍然比普通 SPL token 结算更容易积累边界复杂度。
* **部分事件数据带估算性质**: decrease 路径中的 settled/reward 字段存在按比例拆分的近似过程，链下使用时需注意口径。

---

## 六、验证记录

本轮复审额外执行了以下验证：

* 交叉核对 `audit/` 目录中既有审计报告、修复决策和当前源码实现
* 运行库测试：

```bash
SECURITY_ADMIN=11111111111111111111111111111111 cargo test -p lp_handler --lib
```

结果： `31 passed; 0 failed`

说明：测试通过证明当前单元测试集可正常运行，但不能替代完整的集成测试与主网场景验证。

---

## 七、总结

| 维度 | 评级 |
|------|------|
| 安全架构 | 良好 |
| 前序审计修复完成度 | 良好 |
| 代码质量 | 良好 |
| 新发现风险 | 无新增有效问题 |

本轮复审未发现新的 `Critical`、`High`、`Medium` 或 `Low` 级有效漏洞。  
上一版复审中保留的 `NEW-01` 已完成修复并通过单元测试验证。

**结论**: 当前合约在既定业务前提下可评为 `低风险`，本轮复审后未发现仍需保留的新增有效问题，具备继续推进测试与上线准备的条件。
