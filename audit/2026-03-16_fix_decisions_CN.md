# DeGate LP Handler 审计问题处理决定表

说明：

* 本文档按审计报告中的编号逐项列出处理决定。
* `Fixed` 表示已修复；其中 `059613e` 为既有提交，其余本轮修复已整理为提交 `09841f3039774e285039c0c478dc49e5c77e63e3` 。
* `Acknowledged` 表示已确认但选择不按报告建议修复，并附上接受风险说明。

| 问题 | 处理决定 | Commit Hash | 说明 |
| --- | --- | --- | --- |
| `H01` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已将 program-owned account 的 lamports 检查改为“入口快照 / 出口相对校验”，不再使用绝对 rent 上限，避免外部向 PDA 打入 lamports 导致全局 DoS。 |
| `M01` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已在 deposit / zap 侧按 Token-2022 transfer fee 口径扣减后再计算 liquidity；在 decrease 路径中也按扣费后的 expected principal 与真实余额变化对账。 |
| `M02` | `Fixed` | `059613e` | 已引入 `quoted_mode` 与 `quoted_sqrt_price_x64` 校验，避免 out-of-range 自动换币覆盖用户提供的最小成交保护。 |
| `M03` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已将主 zap swap 改为同时使用 `quoted_mode` / `quoted_sqrt_price_x64` / `swap_min_out` 与 `sqrt_price_limit_x64` tick-boundary enforcement，避免自动换币把价格继续推进穿过仓位边界。 `059613e` 完成了报价锚定，这一轮补上了边界限价。 |
| `M04` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `init_security_config` 现在仍然要求固定初始化管理员，但该管理员不再手写死在仓库里，而是由发布脚本在 `anchor build` 前通过 `SECURITY_ADMIN` 环境变量注入为当前部署钱包地址。这样既满足受限初始化要求，也适配现有发布流程。 |
| `M05` | `Fixed` | `059613e` | 已为 claim / convert 相关路径补充报价约束与价格保护，使其和 deposit 路径保持一致。 |
| `L01` | `Acknowledged` | `-` | 我们已经将 `remaining_accounts` 拆分为 `main_swap_remaining` 、 `action_remaining` 、 `cleanup_swap_remaining_input_token0` 、 `cleanup_swap_remaining_input_token1` 四段；cleanup swap 由链下同时提供双向候选 slice，链上再按实际 leftover 输入方向选择，不再直接复用主 swap 路径。同时，cleanup swap 现已调整为 best-effort：若主 swap 与开仓/加仓已成功，但 cleanup 因 CU、路径或状态偏差失败，则不回滚主流程，leftover 保留在用户账户。当前客户端仍未完整复刻 `post-action liquidity` 对后续 tick array 路径的影响，因此这项未按报告最严格标准做成完全修复；我们接受这部分残余风险。 |
| `L02` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已将 dust 判断改为基于目标侧估算输出，而不是把任意 mint 的 raw amount 直接与 `MIN_USDC_SWAP_AMOUNT` 比较。 |
| `L03` | `Acknowledged` | `-` | 该分支是保留的业务设计：当预估兑换到目标侧的数量过小、可能导致 0-output 或无意义 swap 时，直接将该 reward 输入转入 fee 地址。我们不按报告建议改成仅按 `fee_percent` 收费，因为那会改变既有产品语义；相应风险为已知且可接受。 |
| `L04` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已强制 `fee_token0_account` / `fee_token1_account` 必须为 `fee_owner` 对应 mint 的 canonical ATA，避免 fee 碎片化。 |
| `L05` | `Acknowledged` | `-` | 我们已放宽 `reward-only claim` ，避免在 claim-only 场景因 `NoBalanceChange` 回滚；但不会按报告建议对额外 farming / incentive reward 计费。原因是协议设计上只对池子两币收取手续费，额外激励代币不属于协议收费范围，因此放弃这部分 fee revenue 是有意识接受的产品决策。 |
| `E01` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已将 `USDC_MIN` 统一更名为更准确的 `USDC_MINT` 。 |
| `E02` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已增加统一 helper，统一处理 SPL Token native mint 与 Token-2022 native mint 的识别逻辑。 |
| `E03` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已删除未被使用的 `collect_accounts_to_check` 死代码，避免误导阅读者误以为安全层已统一扫描全部 `remaining_accounts` 。 |
| `E04` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已统一 tick array 相关约束： `swap_and_deposit` / `increase_liquidity` / `decrease_liquidity` 的账户校验逻辑现已与底层 Raydium CPI 语义保持一致；其中 `swap_and_deposit` 已放宽为允许传入尚未初始化的 lower/upper tick array PDA，并在本程序内按 Raydium 规则校验其 PDA 地址，避免合法开仓因包装层过严而被提前拒绝。 |
| `E05` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已在 `init_security_config` 与 `update_security_config` 中强制 `fee_owners` 白名单不能为空。 |
| `E06` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已在 `increase_liquidity` 中校验传入的 `tick_lower_index` / `tick_upper_index` 必须与现有 `personal_position` 一致。 |
| `E07` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | 已将 `convert_to_usdc` 更名为更准确的 `convert_to_target_mint` ，并同步更新相关文档说明。 |

## 示例修改代码

以下代码片段仅作为“修复方式示例”，用于说明我们对关键问题的处理方向；最终以实际提交的完整 diff 为准。

### `H01` program-owned lamports 改为入口快照 / 出口相对校验

文件： `programs/lp_handler/src/security/mod.rs`

```rust
#[derive(Clone, Debug)]
pub struct SecuritySnapshot {
    pub entries: Vec<SnapshotEntry>,
    pub program_owned_lamports: Vec<(Pubkey, u64)>,
}

fn program_owned_lamports_ok(
    accounts: &[AccountInfo<'_>],
    before: &[(Pubkey, u64)],
) -> bool {
    for (key, lamports_before) in before.iter() {
        let Some(ai) = accounts.iter().find(|a| a.key() == *key) else {
            return false;
        };
        if ai.lamports() > *lamports_before {
            return false;
        }
    }
    true
}
```

### `M01` 在 token 计算中纳入 Token-2022 transfer fee

文件： `programs/lp_handler/src/state/utils.rs`

```rust
pub fn get_transfer_fee_for_amount(
    mint: &InterfaceAccount<Mint>,
    pre_fee_amount: u64,
) -> Result<u64> {
    if mint.to_account_info().owner == &anchor_spl::token::ID {
        return Ok(0);
    }

    get_transfer_fee_from_mint_info(mint.to_account_info(), pre_fee_amount)
}
```

文件： `programs/lp_handler/src/instructions/zap_common.rs`

```rust
let amount_0_after_transfer_fee = amount_0_max
    .checked_sub(utils::get_transfer_fee_for_amount(accounts.vault_0_mint(), amount_0_max)?)
    .ok_or(LpDepositError::MathOverflow)?;
let amount_1_after_transfer_fee = amount_1_max
    .checked_sub(utils::get_transfer_fee_for_amount(accounts.vault_1_mint(), amount_1_max)?)
    .ok_or(LpDepositError::MathOverflow)?;
```

### `M03` 主 zap swap 增加 `sqrt_price_limit_x64` 边界保护

文件： `programs/lp_handler/src/instructions/zap_common.rs`

```rust
fn derive_main_swap_price_limit(
    tick_lower_index: i32,
    tick_upper_index: i32,
    swap_input_is_token0: bool,
) -> Result<u128> {
    if swap_input_is_token0 {
        get_sqrt_price_at_tick(tick_lower_index)
    } else {
        get_sqrt_price_at_tick(tick_upper_index)
    }
}
```

```rust
let main_swap_price_limit_x64 =
    derive_main_swap_price_limit(tick_lower_index, tick_upper_index, swap_input_is_token0)?;

swap_v2_common(
    accounts,
    swap_amount_in,
    swap_min_out,
    main_swap_price_limit_x64,
    swap_input_is_token0,
    swap_remaining_slice.to_vec(),
)?;
```

### `M04` 由部署脚本在编译期注入 `SECURITY_ADMIN`

文件： `programs/lp_handler/src/instructions/security_config.rs`

```rust
fn validate_security_config_init_authority(authority: Pubkey) -> Result<()> {
    require!(
        authority == crate::consts::SECURITY_ADMIN,
        LpDepositError::SecurityConfigAdminUnauthorized
    );
    Ok(())
}
```

文件： `programs/lp_handler/build.rs`

```rust
let security_admin = env::var("SECURITY_ADMIN")
    .expect("SECURITY_ADMIN must be set before building lp_handler");
```

文件： `scripts/deploy.ts`

```ts
const buildEnv = buildAnchorBuildEnv(userWallet.publicKey.toBase58());
// 部署脚本在执行 anchor build 时显式传入 buildEnv
// 以便将 SECURITY_ADMIN 注入编译环境
runAnchorBuild(buildEnv);
```

### `L02` dust 判断改为基于目标侧估算输出

文件： `programs/lp_handler/src/instructions/decrease_liquidity.rs`

```rust
fn should_skip_claim_only_dust_swap(
    principal_other_in: u64,
    swap_other_amount_threshold: u64,
) -> bool {
    principal_other_in == 0 && swap_other_amount_threshold < crate::consts::MIN_USDC_SWAP_AMOUNT
}
```

### `L04` 强制 fee token account 为 canonical ATA

文件： `programs/lp_handler/src/instructions/decrease_liquidity.rs`

```rust
fn validate_fee_token_account_keys(
    fee_owner: Pubkey,
    fee_token0_account: Pubkey,
    fee_token1_account: Pubkey,
    vault0_mint: Pubkey,
    vault1_mint: Pubkey,
    vault0_token_program: Pubkey,
    vault1_token_program: Pubkey,
) -> Result<()> {
    let expected_fee_token0 =
        utils::derive_ata_address(&fee_owner, &vault0_mint, &vault0_token_program);
    let expected_fee_token1 =
        utils::derive_ata_address(&fee_owner, &vault1_mint, &vault1_token_program);
    require!(fee_token0_account == expected_fee_token0, LpDepositError::InvalidFeeTokenAccount);
    require!(fee_token1_account == expected_fee_token1, LpDepositError::InvalidFeeTokenAccount);
    Ok(())
}
```

### `E03` 删除未使用的死代码

文件： `programs/lp_handler/src/security/mod.rs`

```rust
// 已删除 collect_accounts_to_check(...)
```

### `E06` 校验 `increase_liquidity` 的 tick 参数必须与仓位一致

文件： `programs/lp_handler/src/instructions/increase_liquidity.rs`

```rust
fn validate_position_ticks_match(
    personal_position: &PersonalPositionState,
    tick_lower_index: i32,
    tick_upper_index: i32,
) -> Result<()> {
    require!(
        personal_position.tick_lower_index == tick_lower_index,
        LpDepositError::InvalidTickRange
    );
    require!(
        personal_position.tick_upper_index == tick_upper_index,
        LpDepositError::InvalidTickRange
    );
    Ok(())
}
```

### 补充： `swap_and_deposit` 兼容未初始化 tick array PDA

文件： `programs/lp_handler/src/instructions/swap_and_deposit.rs`

```rust
fn validate_open_position_tick_array_keys(
    pool_state: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
    tick_spacing: u16,
    tick_array_lower: Pubkey,
    tick_array_upper: Pubkey,
) -> Result<()> {
    let tick_array_lower_start_index =
        TickArrayState::get_array_start_index(tick_lower_index, tick_spacing);
    let tick_array_upper_start_index =
        TickArrayState::get_array_start_index(tick_upper_index, tick_spacing);

    let expected_lower = derive_tick_array_key(pool_state, tick_array_lower_start_index);
    let expected_upper = derive_tick_array_key(pool_state, tick_array_upper_start_index);

    require_keys_eq!(tick_array_lower, expected_lower, LpDepositError::InvalidRemainingAccounts);
    require_keys_eq!(tick_array_upper, expected_upper, LpDepositError::InvalidRemainingAccounts);
    Ok(())
}
```

说明：Raydium 的 `open_position_with_token22_nft` 允许 lower / upper tick array 以“尚未初始化但 PDA 地址正确”的形式传入，并在 CPI 过程中按需要初始化。此前我们的 `swap_and_deposit` 包装层把这两个账户提前收紧为 `AccountLoader<TickArrayState>` ，会把这类合法开仓提前拒绝。现已改为允许未初始化 PDA 通过外层校验，同时保留 PDA 地址校验，确保既兼容底层语义又不放弃账户正确性约束。

### `L01` cleanup swap 使用双向候选账户切片

文件： `programs/lp_handler/src/instructions/zap_common.rs`

```rust
let (
    swap_remaining_slice,
    action_remaining_slice,
    cleanup_swap_remaining_input_token0_slice,
    cleanup_swap_remaining_input_token1_slice,
) = split_zap_remaining_accounts(remaining_accounts)?;

let cleanup_swap_remaining = select_cleanup_remaining_accounts(
    cleanup_swap_remaining_input_token0_slice,
    cleanup_swap_remaining_input_token1_slice,
    actual_cleanup_input_is_token0,
);
```

说明：当前实现已不再直接复用主 swap 的 `swap_remaining` 给 cleanup swap；客户端会同时生成 “cleanup 输入 token0” 与 “cleanup 输入 token1” 两套候选路径，链上再按实际 leftover 方向择一使用。当前客户端为两套候选路径报价时，使用的是“按 cleanup 输入方向分别取对应侧可投入上界”的保守口径，而不是把 `amountIn` 当成精确退款量；这样可以降低因链下低估 leftover 导致缺 tick arrays 的概率。此外，cleanup swap 已调整为 best-effort：一旦主 swap 与开仓/加仓完成，即使 cleanup swap 后续失败，主流程也不会回滚，leftover 会保留在用户账户。但如上文所述，客户端仍未完整复刻 `post-action liquidity` 对路径选择的影响，因此该项仍归类为 `Acknowledged` 而非 `Fixed` 。

补充：安全层入口由各指令入口显式传入 `expected_separator_count` ；当前 `swap_and_deposit` / `increase_liquidity` 传 `3` ， `decrease_liquidity` 传 `1` ，避免不同协议之间误放行或误拒绝。
