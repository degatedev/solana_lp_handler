# LP Handler Mainnet 生产部署与运维手册

本文档只覆盖 `mainnet` 实际生产操作，面向研发、DevOps 和运营协作使用。

本文档基于仓库当前实现整理，关键执行入口如下：

* 合约入口：[programs/lp_handler/src/lib.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/lib.rs)
* 部署脚本：[scripts/deploy.ts](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/deploy.ts)
* 安全配置脚本：[scripts/security_config.ts](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/security_config.ts)
* 程序关闭脚本：[scripts/closeProgram.ts](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/closeProgram.ts)
* 编译期管理员注入：[programs/lp_handler/build.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/build.rs)

## 1. 快速执行清单

先看这一节，再去后面的详细说明。

### 1.1 开发执行清单

发布新版本时，开发需要完成：

* 确认本次变更只涉及业务逻辑，不修改固定的入口检查和出口检查
* 确认本次代码版本可以发布
* 配合 DevOps 确认本次发布对应的合约版本和 `Program ID`
* 观察期如果出问题，负责定位代码或接入层问题
* 如果需要回切，确认旧 `Program ID` 的调用路径仍可恢复

只更新池子或手续费地址时，开发通常不执行链上操作，但需要确认：

* 调用方是否缓存池子列表
* 调用方是否展示或依赖手续费接收地址
* 是否还存在写死的旧配置

### 1.2 DevOps执行清单

每次生产操作前，DevOps 先确认：

* 当前 shell 已加载正确 `.env`
* `ANCHOR_WALLET` 指向正确生产钱包
* `ANCHOR_PROVIDER_URL` 指向可用主网 RPC
* `SECURITY_POOLS` 和 `SECURITY_FEE_OWNERS` 是完整正确列表
* 本次操作目标 `Program ID` 没填错

首次部署或升级发布时，DevOps 执行：

* 确认 `scripts/security_boundary.sha256` 是当前认可的基准值
* 运行 `yarn cli`
* 选择 `部署合约(会自动初始化安全配置)`
* 记录新的 `Program ID`
* 执行链上核验
* 配合开发完成切流

只更新池子或手续费地址时，DevOps 执行：

* 修改 `.env`
* 运行 `yarn cli`
* 选择 `更新安全配置`
* 输入当前生效合约的 `Program ID`

旧合约下线时，DevOps 执行：

* 确认测试和运营都同意下线
* 运行 `yarn cli`
* 选择 `关闭旧合约并回收租金(会自动关闭安全配置)`

### 1.3 测试执行清单

发布前，测试需要完成：

* 回归测试
* 确认本次发布没有改动固定入口/出口检查逻辑
* 确认冒烟用例和观察项
* 确认如果回切到旧 `Program ID`，验证路径仍然可执行

首次部署或升级发布后，测试需要完成：

* 验证新 `Program ID` 上至少一条主流程可用
* 验证开仓、加仓、减仓、领奖中的关键路径
* 观察是否出现 `SecurityPoolNotAllowed`
* 观察是否出现 `InvalidFeeOwner`
* 给出“是否允许关闭旧合约”的明确结论

只更新池子时，测试需要完成：

* 验证旧池子未误删
* 验证新池子可正常通过安全校验

只更新手续费地址时，测试需要完成：

* 验证新的 `fee_owner` 可用
* 验证旧地址是否按预期失效

### 1.4 运营执行清单

每次生产操作前，运营需要完成：

* 确认池子清单
* 确认手续费接收地址清单
* 确认发布时间窗口
* 确认需要通知的业务方

升级发布时，运营需要完成：

* 通知业务侧切流时间
* 在观察期收集业务反馈
* 明确是否还需要保留旧合约作为回滚入口

只更新池子或手续费地址时，运营需要完成：

* 提供最终生效名单
* 确认业务何时开始使用新配置
* 反馈业务侧是否仍有异常

### 1.5 常见操作入口

首次部署或新版本发布：

```text
yarn cli
# 选择：部署合约(会自动初始化安全配置)
```

说明：

* 部署脚本开始时会自动校验固定安全边界文件摘要
* 如果 [scripts/security_boundary.sha256](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/security_boundary.sha256) 与当前文件内容不匹配，部署会直接终止

更新池子白名单或手续费接收地址：

```text
yarn cli
# 选择：更新安全配置
```

只初始化安全配置：

适用于 CLI 部署失败后，已经手动完成程序部署，只需要单独初始化 `security_config` 的场景。

```text
yarn cli
# 选择：初始化安全配置
```

关闭旧合约：

```text
yarn cli
# 选择：关闭旧合约并回收租金(会自动关闭安全配置)
```

## 2. 角色分工总览

本文档默认涉及 4 类角色：

* `开发`：负责合约业务代码变更、合约侧问题定位、发布前变更范围确认
* `DevOps`：负责生产环境、部署钱包、`.env`、部署命令、链上配置更新、旧合约关闭、发布记录
* `测试`：负责回归测试、冒烟验证、观察期确认、问题复现
* `运营`：负责确认池子清单、手续费接收地址清单、发布时间窗口、发布通知、收集业务反馈

各角色重点关注章节：

* `开发`：重点看 `快速执行清单` `合约升级发布` `下线旧合约与回收租金` `生产变更前检查清单` `生产事故高发点`
* `DevOps`：重点看 `快速执行清单` `生产环境准备` `首次部署` `合约升级发布` `更新池子白名单` `更新手续费接收地址` `下线旧合约与回收租金` `生产变更前检查清单`
* `测试`：重点看 `快速执行清单` `首次部署` `合约升级发布` `更新池子白名单` `更新手续费接收地址` `下线旧合约与回收租金` `生产变更前检查清单`
* `运营`：重点看 `快速执行清单` `合约升级发布` `更新池子白名单` `更新手续费接收地址` `下线旧合约与回收租金` `生产变更前检查清单`

如果只是更新池子或手续费地址，不涉及代码发布：

* `运营` 提供最终业务清单
* `DevOps` 执行链上更新
* `测试` 执行更新后验证
* `开发` 仅在合约业务逻辑或合约侧问题排查需要时介入

## 3. 合约运维模型

### 3.1 这份合约怎么部署

本项目不是传统的“原 Program ID 原地升级”模式。

当前仓库的发布脚本会执行以下动作：

1. 重新生成 `target/deploy/lp_handler-keypair.json`
2. 执行 `anchor keys sync`
3. 以当前钱包地址注入编译期 `SECURITY_ADMIN`
4. `anchor build`
5. `anchor deploy --provider.cluster mainnet`
6. 立刻调用 `init_security_config`

这意味着：

* 每次“升级发布”都会产生一个新的 `Program ID`
* 后端服务和运维脚本都必须切换到新的 `Program ID`
* 新老合约可以并存一段观察期
* 观察期结束后，再关闭旧合约并回收租金

### 3.2 链上有哪些管理对象

生产运维重点只需要理解两个对象：

* `Program`：合约程序本身
* `security_config` PDA：安全配置账户，PDA seed 固定为 `b"security_config"`

`security_config` 中保存两类白名单：

* `pools`：允许操作的 Raydium CLMM 池子地址，最多 40 个
* `fee_owners`：允许作为手续费接收方的地址，最多 6 个

对应实现见：

* [programs/lp_handler/src/state/security_config.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/state/security_config.rs)
* [programs/lp_handler/src/instructions/security_config.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/instructions/security_config.rs)

### 3.3 谁可以管理安全配置

`security_config` 初始化时，调用人必须等于编译期常量 `SECURITY_ADMIN` 。

这个常量来自构建时环境变量 `SECURITY_ADMIN` ，由 [programs/lp_handler/build.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/build.rs) 生成。

发布脚本默认把“当前部署钱包地址”注入为 `SECURITY_ADMIN` 。因此生产上要遵守这条规则：

* 用哪个钱包执行部署，这个钱包就会成为该版本合约的 `security_config.authority`
* 后续 `update_security_config` 和 `close_security_config` 也必须继续使用同一个钱包

## 4. 生产环境准备

### 4.1 本机要求

* Solana CLI `3.0.11`
* Anchor CLI `0.31.1`
* Rust `1.89.0`
* Node.js `18 LTS` 或 `20 LTS`
* Yarn

版本说明：

* Rust 版本以仓库 [rust-toolchain.toml](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/rust-toolchain.toml) 为准，当前固定为 `1.89.0`
* Node.js 版本仓库中没有通过 `package.json` 的 `engines` 字段强制锁定，但考虑到当前依赖组合，生产环境建议使用 `18 LTS` 或 `20 LTS`
* Anchor 版本以 [Anchor.toml](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/Anchor.toml) 为准，当前为 `0.31.1`

### 4.2 钱包与网络

生产钱包需要满足：

* 能访问 `mainnet-beta`
* 钱包私钥路径可被 `ANCHOR_WALLET` 正确读取
* 钱包里有足够 SOL 支付构建、部署和账户租金

建议先执行：

```bash
solana config get
solana config set --url mainnet-beta
solana address
solana balance
```

### 4.3 `.env` 配置

项目根目录需要准备 `.env` 。

最小配置如下：

```env
ANCHOR_WALLET=~/.config/solana/id.json
ANCHOR_PROVIDER_URL=https://your-mainnet-rpc
SECURITY_POOLS=pool_pubkey_1,pool_pubkey_2
SECURITY_FEE_OWNERS=fee_owner_pubkey_1
```

要求如下：

* `ANCHOR_WALLET` 必须是本次生产操作的钱包
* `ANCHOR_PROVIDER_URL` 必须指向可用的主网 RPC
* `SECURITY_POOLS` 不能为空
* `SECURITY_FEE_OWNERS` 不能为空
* 池子最多 40 个
* 手续费接收地址最多 6 个
* 地址之间使用英文逗号分隔，不能带空格

## 5. 首次部署

### 5.1 适用场景

首次部署适用于：

* 主网首次上线
* 创建一个全新的生产合约地址

角色分工：

* `开发`：确认代码版本可发布，确认调用方如何接入新 `Program ID`
* `DevOps`：执行部署、初始化安全配置、记录 Program ID
* `测试`：部署后做链上和业务冒烟验证
* `运营`：确认上线窗口、确认初始池子与手续费地址清单

### 5.2 推荐做法

直接使用 CLI：

```text
yarn cli
# 选择：部署合约(会自动初始化安全配置)
```

执行前确认：

* [scripts/security_boundary.sha256](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/security_boundary.sha256) 已更新为当前认可的安全基线
* 如果部署脚本提示固定安全边界摘要不匹配，应先停止发布并检查 [programs/lp_handler/src/lib.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/lib.rs) 和 [programs/lp_handler/src/security/mod.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/security/mod.rs)

### 5.3 实际发生的动作

执行后脚本会按顺序完成：

1. 生成新的 Program Keypair
2. 把新的 Program ID 同步到 `Anchor.toml` 和源码相关产物
3. 使用当前钱包地址作为 `SECURITY_ADMIN` 编译
4. 部署到 `mainnet`
5. 用 `.env` 中的 `SECURITY_POOLS` 与 `SECURITY_FEE_OWNERS` 初始化 `security_config`
6. 打印 `solana program show <PROGRAM_ID>` 信息

### 5.4 首次部署后的核对项

部署完成后，至少做以下核对：

1. 记录新的 `Program ID`
2. 确认部署钱包地址
3. 确认 `.env` 中白名单与预期一致
4. 确认 `security_config` 已初始化成功
5. 确认应用侧尚未误切到旧地址或错误地址

建议执行：

```bash
solana program show <PROGRAM_ID>
solana account <SECURITY_CONFIG_PDA>
```

`SECURITY_CONFIG_PDA` 的推导规则是：

* seed 固定为 `security_config`
* program id 为当前新部署合约地址

如果不想手工推导，最稳妥的做法是直接通过 CLI 初始化和更新，不手写 PDA。

职责建议：

* `开发`：确认接入方配置项已准备好接收新 `Program ID`
* `DevOps`：完成链上账户核对和发布记录归档
* `测试`：至少验证一次开仓、加仓或减仓主流程
* `运营`：确认业务侧已知晓何时开始使用新地址

## 6. 合约升级发布

### 6.1 升级的本质

本仓库里的“升级”实际是“重新部署一个新合约版本”，不是对旧 `Program ID` 做原地升级。

所以一次完整升级包含两段：

1. 发布新合约并初始化其安全配置
2. 业务流量切换到新合约，观察稳定后关闭旧合约

另外，当前团队的升级约束是：

* `入口检查` 和 `出口检查` 属于固定安全边界
* 常规升级只允许更新业务代码
* 如果变更涉及入口/出口安全层，则不属于常规升级，需要单独评审

### 6.2 固定入口/出口检查逻辑

当前固定安全边界的核心位置如下：

* [programs/lp_handler/src/lib.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/lib.rs) 中的 `secure_entrypoint!`
* [programs/lp_handler/src/security/mod.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/security/mod.rs) 中的 `entry_check_and_snapshot`、`exit_check`、`resolve_policy`

发布前必须确认以下逻辑未被修改：

* 各业务指令仍然通过 `secure_entrypoint!` 包装执行
* `entry_check_and_snapshot` 的调用顺序没有变化
* `exit_check` 的调用顺序没有变化
* `resolve_policy` 没有被替换或绕过
* `security/mod.rs` 中关于 delegate、close authority、program-owned lamports、未初始化账户、白名单读取的检查逻辑没有被弱化

允许变更的范围应当限制在业务代码层，例如：

* `programs/lp_handler/src/instructions/` 下的业务处理逻辑
* 业务参数、业务路径、业务计算细节

如果本次变更触及以下内容，必须视为非常规升级：

* `secure_entrypoint!` 宏本身
* `security/mod.rs` 的入口检查或出口检查
* `init_security_config`、`update_security_config`、`close_security_config` 的权限模型
* 白名单校验、安全策略、账户权限约束

### 6.3 标准升级步骤

角色分工：

* `开发`：负责本次合约代码变更、发布后问题定位
* `DevOps`：负责部署、链上核验、环境配置变更、旧合约关闭
* `测试`：负责升级前回归和升级后冒烟，确认是否允许进入下线阶段
* `运营`：负责发布时间窗口、业务通知、观察期反馈

#### 步骤 1：确认发布窗口

发布前确认：

* 调用方已经具备切换新 `Program ID` 的发布条件
* 当前生产池子和手续费接收地址清单已经确认
* 旧合约仍保留，可作为回滚锚点

执行责任：

* `开发`：先确认本次变更没有修改固定入口/出口检查逻辑
* `开发`：确认本次合约代码版本可发布
* `DevOps`：确认生产环境和回滚路径
* `测试`：确认回归完成
* `运营`：确认发布窗口和通知范围

#### 步骤 2：更新新版本代码并检查 `.env`

重点确认：

* `.env` 中 `SECURITY_POOLS` 包含升级后要继续支持的全部池子
* `.env` 中 `SECURITY_FEE_OWNERS` 包含升级后允许使用的全部手续费接收地址
* 当前 `ANCHOR_WALLET` 就是预期的新版本安全配置管理员

执行责任：

* `开发`：确认本次代码版本与调用方改动一致
* `开发`：确认变更范围仅在业务代码层，没有改动 `secure_entrypoint!` 或 `security/mod.rs`
* `DevOps`：确认生产配置和管理员钱包正确
* `运营`：确认业务清单是最终版本

#### 步骤 3：部署新合约

执行：

```text
yarn cli
# 选择：部署合约(会自动初始化安全配置)
```

说明：

* 部署脚本会先自动校验固定安全边界文件的汇总 `sha256`
* 若校验失败，说明入口/出口固定安全边界可能被改动，本次发布应终止并转为非常规升级评审

#### 步骤 4：记录发布结果

至少记录：

* 新 `Program ID`
* 部署时间
* 操作人
* 部署钱包地址
* 初始化时使用的池子白名单
* 初始化时使用的手续费接收地址白名单

#### 步骤 5：先做链上核验

建议执行：

```bash
solana program show <NEW_PROGRAM_ID>
```

并确认：

* 程序账户存在
* 部署成功
* 可查询到程序信息

执行责任：

* `DevOps`：完成链上核验
* `测试`：准备在新地址上执行冒烟验证

#### 步骤 6：切换调用方到新 `Program ID`

需要切换的对象通常包括：

* 后端服务配置
* 运维脚本

这一步必须作为正式发布动作记录下来，因为只部署新合约但不切换调用方，不算发布完成。

执行责任：

* `DevOps`：发布配置并执行切流
* `测试`：切流后立即开始冒烟验证
* `运营`：同步业务侧切流开始时间

#### 步骤 7：观察期

切流后建议观察以下内容：

* 新请求是否都命中新 `Program ID`
* 用户开仓、加仓、减仓、领奖是否正常
* 是否出现 `SecurityPoolNotAllowed`
* 是否出现 `InvalidFeeOwner`
* 是否出现由于白名单遗漏导致的业务失败

如果观察期发现问题，而旧合约仍保留，可以先把调用方切回旧 `Program ID` ，再排查新合约版本。

执行责任：

* `测试`：给出观察期是否通过的结论
* `运营`：反馈真实业务是否异常
* `开发`：定位新版本问题或准备回切后的修复
* `DevOps`：执行保持新版本或回切旧版本

#### 步骤 8：关闭旧合约

确认所有流量都已切到新版本并观察稳定后，执行：

```text
yarn cli
# 选择：关闭旧合约并回收租金(会自动关闭安全配置)
```

脚本会先关闭旧版本 `security_config` ，再执行：

```bash
solana program close <OLD_PROGRAM_ID> --recipient <当前钱包> --bypass-warning
```

执行责任：

* `测试`：确认新版本稳定，不再需要依赖旧地址回滚
* `运营`：确认业务已经完全切流
* `DevOps`：执行旧合约关闭和租金回收

### 6.4 升级发布的回滚原则

如果新版本发布后发现问题：

* 只要旧合约还没关闭，优先回切调用方到旧 `Program ID`
* 不要急着关闭旧合约
* 修复问题后重新发布一个新的 Program ID

注意：

* 关闭旧合约是不可逆操作
* 一旦执行 `solana program close`，旧地址不能继续承接生产流量

## 7. 更新池子白名单

### 7.1 适用场景

当新增、下线或调整允许使用的 Raydium CLMM 池子时，需要更新 `security_config.pools` 。

角色分工：

* `运营`：提供最终池子清单
* `DevOps`：更新 `.env` 并执行链上更新
* `测试`：验证旧池子未误删、新池子可用
* `开发`：如调用方对池子列表有缓存或配置依赖，负责同步修改

### 7.2 实际机制

脚本不会做“增量追加”，而是用 `.env` 里的完整列表整体覆盖链上配置。

这意味着：

* 修改前必须先整理出“更新后的完整池子列表”
* 如果只追加一个池子，但忘了把原有池子也保留在 `.env` 中，链上旧池子会一并失效

### 7.3 操作步骤

#### 步骤 1：准备新的完整池子列表

确认：

* 新池子地址正确
* 老池子中仍需保留的地址也都在列表中
* 总数量不超过 40 个

#### 步骤 2：修改 `.env`

更新：

```env
SECURITY_POOLS=pool_a,pool_b,pool_c
```

#### 步骤 3：执行更新

```text
yarn cli
# 选择：更新安全配置
```

随后输入当前生效合约的 `Program ID` 。

#### 步骤 4：更新后验证

至少验证两件事：

1. 原来应当继续可用的池子没有被误删
2. 新池子可以正常通过安全校验

业务层验证建议：

* 用新增池子执行一次 `swap_and_deposit`
* 若报 `SecurityPoolNotAllowed`，说明白名单未生效或池子地址填写错误

执行责任：

* `测试`：完成业务验证
* `运营`：确认新增池子可以对外使用
* `开发`：若业务侧仍报错，配合排查是否还有调用方配置未更新

### 7.4 池子更新注意事项

* 生产上不要把 `.env` 当作“差量补丁”，它是“全量配置源”
* 更新前先备份旧值
* 更新后立刻做业务验证，不要只看链上交易成功

## 8. 更新手续费接收地址

### 8.1 适用场景

当需要新增、下线或替换手续费接收地址时，需要更新 `security_config.fee_owners` 。

角色分工：

* `运营`：提供最终手续费接收地址清单
* `DevOps`：更新 `.env` 并执行链上更新
* `测试`：验证新地址可用、旧地址是否按预期失效
* `开发`：如调用方对手续费地址有额外配置依赖，负责同步修改

### 8.2 实际机制

和池子白名单一样，这里也是全量覆盖，不是增量追加。

因此：

* `.env` 里必须填入更新后的完整 `fee_owner` 列表
* 如果遗漏旧地址，旧地址会立即失去权限

### 8.3 操作步骤

#### 步骤 1：准备新的完整 `fee_owner` 列表

确认：

* 地址正确
* 仍需继续使用的旧地址没有漏掉
* 总数量不超过 6 个

#### 步骤 2：修改 `.env`

更新：

```env
SECURITY_FEE_OWNERS=fee_owner_a,fee_owner_b
```

#### 步骤 3：执行更新

```text
yarn cli
# 选择：更新安全配置
```

随后输入当前生效合约的 `Program ID` 。

#### 步骤 4：更新后验证

建议使用新的 `fee_owner` 做一次真实业务验证：

* 执行一次 `decrease_liquidity`
* 确认不报 `InvalidFeeOwner`
* 确认手续费接收 ATA 与预期地址一致

执行责任：

* `测试`：完成手续费路径验证
* `运营`：确认业务侧启用哪个手续费地址
* `开发`：若调用方展示或配置依赖该地址，确认已同步

## 9. 下线旧合约与回收租金

### 9.1 什么时候可以下线

只有在以下条件全部满足后，才可以关闭旧合约：

1. 所有调用方都已经切到新 `Program ID`
2. 观察期内新版本没有异常
3. 不再需要使用旧合约做生产回滚

角色分工：

* `开发`：确认调用方没有残留旧 `Program ID`
* `测试`：确认观察期通过，不再需要回滚
* `运营`：确认业务已完成切流且不再依赖旧地址
* `DevOps`：执行关闭和回收租金

### 9.2 实际执行步骤

执行：

```text
yarn cli
# 选择：关闭旧合约并回收租金(会自动关闭安全配置)
```

输入要关闭的旧 `Program ID` 后，脚本会：

1. 调用 `close_security_config`
2. 关闭旧合约程序账户
3. 把租金返还到当前操作钱包

### 9.3 风险提示

* 该操作不可逆
* 一旦关掉旧合约，发布回滚只能靠“重新部署一个新版本”，不能恢复到被关闭的旧地址
* 如果你还不确定业务是否完全切流，不要执行关闭

## 10. 安全注意事项

### 10.1 钱包安全

* 部署时使用的钱包会成为该版本 `security_config.authority`
* 后续更新 `security_config` 和关闭 `security_config` 也必须继续使用同一个钱包
* 生产部署钱包不要和个人日常开发钱包混用
* `ANCHOR_WALLET` 指向的私钥文件必须确认无误后再执行命令

### 10.2 配置安全

* `.env` 中的 `SECURITY_POOLS` 和 `SECURITY_FEE_OWNERS` 是全量配置源，不是增量补丁
* 任何更新前都要先备份旧 `.env` 配置
* 池子地址和手续费接收地址建议至少做一次人工复核
* 生产变更时不要临时手改不留痕，建议保留变更记录

### 10.3 发布安全

* 当前仓库的升级方式是发布新的 `Program ID`，不是原地升级
* 新版本部署完成不等于发布完成，必须等调用方切流成功
* 旧合约不要过早关闭，只有在观察期通过后再执行下线
* 旧合约未关闭前，才有最快的回切空间

### 10.4 操作安全

* 每次执行前都要再次确认目标 `Program ID`
* 执行 `关闭旧合约并回收租金` 前，必须确认目标是旧合约而不是当前生效合约
* 白名单更新完成后必须做业务验证，不能只看链上交易成功
* 如果你不确定本次变更会影响哪些调用方，先不要执行生产操作

### 10.5 环境安全

* 生产环境尽量固定 Rust、Node.js、Anchor 版本，避免临时升级工具链
* 确认 `ANCHOR_PROVIDER_URL` 指向的是正确的主网 RPC
* 在执行生产命令前，先确认当前 shell 没有残留其他项目的环境变量

### 10.6 固定安全文件的 SHA256 汇总检查

由于当前团队约束是“入口检查和出口检查固定，只允许更新业务代码”，所以每次编译部署前，都必须对固定安全文件做一次摘要校验。

建议纳入校验范围的固定文件：

* [programs/lp_handler/src/lib.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/lib.rs)
* [programs/lp_handler/src/security/mod.rs](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/programs/lp_handler/src/security/mod.rs)

校验思路如下：

1. 按固定顺序分别计算每个文件的 `sha256`
2. 将“文件路径 + 文件 sha256”拼成一份固定文本
3. 再对这份固定文本计算一次 `sha256`
4. 把这个最终结果作为“固定安全边界汇总指纹”

如果这个汇总指纹和基准值不一致，说明固定入口/出口安全边界发生了变化，本次发布不能按常规升级处理。

当前仓库的实际机制是：

* 基准值保存在 [scripts/security_boundary.sha256](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/security_boundary.sha256)
* 部署脚本 [scripts/deploy.ts](http://gitlab.bihutest.com/yukai/solana_lp_handler/-/blob/dev/scripts/deploy.ts) 会在开始阶段自动校验
* 如果校验失败，部署会直接终止

生成或更新基准值时，可使用以下命令：

```bash
{
  shasum -a 256 programs/lp_handler/src/lib.rs
  shasum -a 256 programs/lp_handler/src/security/mod.rs
} | shasum -a 256
```

命令说明：

* 第一层 `shasum -a 256` 分别计算两个固定文件的摘要
* 第二层 `shasum -a 256` 对前面的结果再次计算摘要
* 最终输出的第一列，就是本次固定安全文件的汇总 `sha256`

建议执行方式：

* 先在你们认可的安全基线版本上执行一次，得到基准汇总值
* 将该值写入 `scripts/security_boundary.sha256`
* 后续常规发布时，不需要手工重复执行该命令，部署脚本会自动校验
* 只有在你们明确批准调整固定安全边界时，才应该重新生成并更新该基准值
* 如果部署脚本校验失败，先停止发布，检查是否误改了 `secure_entrypoint!` 或 `security/mod.rs`

## 11. 生产变更前检查清单

每次生产操作前，至少确认以下项目：

* 当前 shell 已加载正确 `.env`
* `ANCHOR_WALLET` 指向正确生产钱包
* `ANCHOR_PROVIDER_URL` 指向可用主网 RPC
* `SECURITY_POOLS` 是完整正确列表
* `SECURITY_FEE_OWNERS` 是完整正确列表
* 本次操作目标 `Program ID` 没填错
* 是否处于允许变更的发布窗口
* 是否已经通知前后端/运营准备切流或观察

按角色拆分的最小检查项：

* `开发`：代码版本、调用方配置、`Program ID` 切流点、回切方式
* `DevOps`：钱包、RPC、`.env`、部署命令、目标 Program ID、回收动作
* `测试`：回归结果、冒烟用例、观察项、回切后的验证路径
* `运营`：池子清单、手续费地址清单、发布时间、通知对象、观察期反馈人

## 12. 生产事故高发点

### 12.1 误以为这是“原地升级”

不是。

当前仓库的升级策略是“发布新 Program ID”，所以所有调用方都要切换地址。

### 12.2 误把 `.env` 当增量更新

不是。

`update_security_config` 会按 `.env` 中的完整列表整体覆盖链上配置。

### 12.3 用错钱包

部署时的钱包会成为该版本 `security_config.authority` 。后续更新和关闭安全配置必须继续使用该钱包。

### 12.4 新版本已部署，但调用方没切流

这种情况下链上已经有新合约，但业务还在跑旧合约，不能算发布完成。

### 12.5 旧合约关得太早

只要旧合约还没关闭，就还有快速回切空间。过早关闭会显著增加发布风险。
