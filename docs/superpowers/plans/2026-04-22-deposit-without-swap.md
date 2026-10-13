# deposit_without_swap 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 `deposit_without_swap` 指令，支持双币直接开仓或追加流动性，完全不执行任何 swap，剩余双币原样退回用户。

**Architecture:** 单一指令，`nft_mint: Option<Pubkey>` 区分开仓（None）和追加（Some），position 相关账户统一使用 `UncheckedAccount` 并在运行时按分支做对应校验。不调用任何 swap CPI，remaining_accounts 直接作为 action_remaining 传给 open_position / increase_liquidity CPI。安全层（`secure_entrypoint!`）、recipient 逻辑、wSOL 处理、事件发射与现有指令完全一致。

**Tech Stack:** Anchor 0.31.1、Rust、Raydium CLMM CPI（raydium-amm-v3）、SPL Token / Token-2022

---

## 文件变更清单

| 操作 | 文件 |
|------|------|
| 新建 | `programs/lp_handler/src/instructions/deposit_without_swap.rs` |
| 修改 | `programs/lp_handler/src/instructions/mod.rs` |
| 修改 | `programs/lp_handler/src/lib.rs` |
| 修改 | `programs/lp_handler/src/state/errors.rs` |

---

## Task 1: 新增 LiquiditySlippageExceeded 错误码

**Files:**
- Modify: `programs/lp_handler/src/state/errors.rs`

- [ ] **Step 1: 在 errors.rs 末尾（`RecipientNotInAccounts` 之后）插入新错误码**

打开 `programs/lp_handler/src/state/errors.rs`，在最后一个错误码 `RecipientNotInAccounts` 后、结束 `}` 之前插入：

```rust
    #[msg("Computed on-chain liquidity deviates from quoted liquidity beyond slippage tolerance")]
    LiquiditySlippageExceeded,
```

最终该段落如下（含上下文）：

```rust
    #[msg("Invalid recipient: recipient pubkey is not present in instruction accounts")]
    RecipientNotInAccounts,

    #[msg("Computed on-chain liquidity deviates from quoted liquidity beyond slippage tolerance")]
    LiquiditySlippageExceeded,
}
```

- [ ] **Step 2: 编译确认无错误**

```bash
cargo test --package lp_handler --lib 2>&1 | tail -5
```

预期输出：`test result: ok.` 或 `0 failed`

---

## Task 2: 创建 deposit_without_swap.rs 骨架

**Files:**
- Create: `programs/lp_handler/src/instructions/deposit_without_swap.rs`
- Modify: `programs/lp_handler/src/instructions/mod.rs`

- [ ] **Step 1: 创建文件，写入完整 Accounts 结构体和空函数体**

创建 `programs/lp_handler/src/instructions/deposit_without_swap.rs`，内容如下：

```rust
use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::libraries::{get_sqrt_price_at_tick, liquidity_math};
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{PersonalPositionState, PoolState, TickArrayState, TICK_ARRAY_SEED};

use crate::{utils, LpDepositError, LpHandlerIncreaseLiquidityEvent, SECURITY_CONFIG_SEED};
use super::zap_common;

#[derive(Accounts)]
#[instruction(
    amount_0_in: u64,
    amount_1_in: u64,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity: u128,
    slippage_bps: u16,
    nft_mint: Option<Pubkey>,
)]
pub struct DepositWithoutSwap<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// CHECK: NFT owner / 退款目标。必须是第二个账户，安全层依赖位置。
    #[account(mut)]
    pub recipient: UncheckedAccount<'info>,

    /// 签名者 / 支付者
    #[account(mut)]
    pub signer: Signer<'info>,

    #[account(
        mut,
        token::mint = token_vault_0.mint,
        token::authority = signer,
    )]
    pub signer_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = signer,
    )]
    pub signer_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_0.mint,
        token::authority = recipient,
    )]
    pub refund_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = recipient,
    )]
    pub refund_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Pool 状态
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(mut)]
    pub fee_owner: SystemAccount<'info>,

    #[account(
        mut,
        token::mint = token_vault_0.mint,
        token::authority = fee_owner,
    )]
    pub fee_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
        token::authority = fee_owner,
    )]
    pub fee_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 池子 token_0 金库
    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 池子 token_1 金库
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program_2022: Program<'info, Token2022>,

    /// CHECK: 安全配置 PDA，seeds+bump 校验
    #[account(seeds = [SECURITY_CONFIG_SEED], bump)]
    pub security_config: UncheckedAccount<'info>,

    #[account(address = token_vault_0.mint)]
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(address = token_vault_1.mint)]
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    // ========== Position 相关账户（UncheckedAccount，运行时校验）==========

    /// 开仓：新 NFT mint keypair（必须为 signer）
    /// 追加：已有 position 的 NFT mint
    #[account(mut)]
    pub position_nft_mint: UncheckedAccount<'info>,

    /// 开仓：NFT 将 mint 到的 token account
    /// 追加：持有 NFT 的 token account（amount=1, authority=signer）
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: protocol_position 已废弃，仅为兼容保留
    pub protocol_position: UncheckedAccount<'info>,

    /// 开仓：由 Raydium 初始化；追加：已存在，需反序列化校验
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

    /// 开仓：可未初始化；追加：必须已初始化
    #[account(mut)]
    pub tick_array_lower: UncheckedAccount<'info>,

    /// 开仓：可未初始化；追加：必须已初始化
    #[account(mut)]
    pub tick_array_upper: UncheckedAccount<'info>,
    // ======== remaining_accounts =========
    // 直接传 action_tick_arrays，无分隔符。
    // ctx.remaining_accounts 整体作为 action_remaining 传给 open_position / increase_liquidity CPI。
}

pub fn deposit_without_swap<'a, 'b, 'c: 'info, 'info>(
    _ctx: Context<'a, 'b, 'c, 'info, DepositWithoutSwap<'info>>,
    _amount_0_in: u64,
    _amount_1_in: u64,
    _tick_lower_index: i32,
    _tick_upper_index: i32,
    _liquidity: u128,
    _slippage_bps: u16,
    _nft_mint: Option<Pubkey>,
) -> Result<()> {
    Ok(())
}
```

- [ ] **Step 2: 在 mod.rs 注册新模块**

打开 `programs/lp_handler/src/instructions/mod.rs`，在文件末尾追加：

```rust
pub mod deposit_without_swap;
pub use deposit_without_swap::*;
```

- [ ] **Step 3: 编译确认骨架无错误**

```bash
cargo test --package lp_handler --lib 2>&1 | tail -5
```

预期输出：`test result: ok.` 或 `0 failed`

---

## Task 3: 实现校验辅助函数和单元测试

**Files:**
- Modify: `programs/lp_handler/src/instructions/deposit_without_swap.rs`

在文件 `pub fn deposit_without_swap` **之前**插入以下所有辅助函数。

- [ ] **Step 1: 插入 validate_liquidity_slippage 函数**

```rust
/// 校验链上重算的 computed_liquidity 与链下 quoted_liquidity 的偏差不超过 slippage_bps。
///
/// 公式：|computed - quoted| * 10_000 <= quoted * slippage_bps
fn validate_liquidity_slippage(
    computed: u128,
    quoted: u128,
    slippage_bps: u16,
) -> Result<()> {
    require!(quoted > 0, LpDepositError::InvalidDepositAmount);
    let delta = if computed >= quoted {
        computed - quoted
    } else {
        quoted - computed
    };
    require!(
        delta.saturating_mul(10_000) <= quoted.saturating_mul(slippage_bps as u128),
        LpDepositError::LiquiditySlippageExceeded
    );
    Ok(())
}
```

- [ ] **Step 2: 插入 validate_position_nft_account 函数**

SPL Token 和 Token-2022 账户的基础字段布局相同（mint: 0-31, owner: 32-63, amount: 64-71）。

```rust
/// 校验 position_nft_account：mint 匹配、authority 为 signer、amount == 1。
fn validate_position_nft_account(
    account: &UncheckedAccount,
    expected_mint: Pubkey,
    expected_owner: Pubkey,
) -> Result<()> {
    let data = account.try_borrow_data()?;
    // SPL Token / Token-2022 基础布局：mint(0..32) owner(32..64) amount(64..72)
    require!(data.len() >= 72, LpDepositError::InvalidPositionNftAccount);
    let mint = Pubkey::try_from(&data[0..32])
        .map_err(|_| error!(LpDepositError::InvalidPositionNftAccount))?;
    let owner = Pubkey::try_from(&data[32..64])
        .map_err(|_| error!(LpDepositError::InvalidPositionNftAccount))?;
    let amount = u64::from_le_bytes(
        data[64..72]
            .try_into()
            .map_err(|_| error!(LpDepositError::InvalidPositionNftAccount))?,
    );
    require_keys_eq!(mint, expected_mint, LpDepositError::InvalidPositionNftAccount);
    require_keys_eq!(owner, expected_owner, LpDepositError::InvalidPositionNftAccount);
    require!(amount == 1, LpDepositError::InvalidPositionNftAccount);
    Ok(())
}
```

- [ ] **Step 3: 插入 validate_personal_position 函数**

```rust
/// 校验 personal_position：owned by Raydium、pool_id 匹配、tick 范围匹配。
fn validate_personal_position(
    account: &UncheckedAccount,
    expected_pool: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
) -> Result<()> {
    require_keys_eq!(
        *account.owner,
        raydium_amm_v3::ID,
        LpDepositError::InvalidRemainingAccounts
    );
    let data = account.try_borrow_data()?;
    let personal_position =
        PersonalPositionState::try_deserialize(&mut data.as_ref())?;
    require_keys_eq!(
        personal_position.pool_id,
        expected_pool,
        LpDepositError::InvalidRemainingAccounts
    );
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

- [ ] **Step 4: 插入 tick array 地址校验辅助函数（复制自 swap_and_deposit.rs 私有函数）**

```rust
fn derive_tick_array_key(pool_state: Pubkey, tick_array_start_index: i32) -> Pubkey {
    Pubkey::find_program_address(
        &[
            TICK_ARRAY_SEED.as_bytes(),
            pool_state.as_ref(),
            &tick_array_start_index.to_be_bytes(),
        ],
        &raydium_amm_v3::ID,
    )
    .0
}

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

    require_keys_eq!(
        tick_array_lower,
        expected_lower,
        LpDepositError::InvalidRemainingAccounts
    );
    require_keys_eq!(
        tick_array_upper,
        expected_upper,
        LpDepositError::InvalidRemainingAccounts
    );
    Ok(())
}
```

- [ ] **Step 5: 在文件末尾插入单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liquidity_slippage_accepts_zero_delta() {
        assert!(validate_liquidity_slippage(1_000_000, 1_000_000, 100).is_ok());
    }

    #[test]
    fn liquidity_slippage_accepts_within_tolerance() {
        // delta = 100, quoted = 1_000_000, slippage = 100 bps (1%)
        // 100 * 10_000 = 1_000_000 <= 1_000_000 * 100 = 100_000_000 → ok
        assert!(validate_liquidity_slippage(1_000_100, 1_000_000, 100).is_ok());
    }

    #[test]
    fn liquidity_slippage_rejects_exceeds_tolerance() {
        // delta = 200, quoted = 1_000_000, slippage = 1 bps
        // 200 * 10_000 = 2_000_000 > 1_000_000 * 1 = 1_000_000 → reject
        let err = validate_liquidity_slippage(1_000_200, 1_000_000, 1).unwrap_err();
        assert_eq!(err, LpDepositError::LiquiditySlippageExceeded.into());
    }

    #[test]
    fn liquidity_slippage_rejects_zero_quoted() {
        let err = validate_liquidity_slippage(1000, 0, 100).unwrap_err();
        assert_eq!(err, LpDepositError::InvalidDepositAmount.into());
    }

    #[test]
    fn liquidity_slippage_accepts_computed_below_quoted() {
        // computed < quoted，delta = 50_000，slippage = 100 bps (1%)
        // 50_000 * 10_000 = 500_000_000 <= 5_000_000 * 100 = 500_000_000 → ok (边界)
        assert!(validate_liquidity_slippage(4_950_000, 5_000_000, 100).is_ok());
    }

    #[test]
    fn tick_array_key_derivation_matches_raydium_formula() {
        let pool = Pubkey::new_unique();
        let tick_spacing = 60u16;
        let tick_lower = -1200i32;
        let tick_upper = 600i32;

        let lower_start = TickArrayState::get_array_start_index(tick_lower, tick_spacing);
        let upper_start = TickArrayState::get_array_start_index(tick_upper, tick_spacing);

        let result = validate_open_position_tick_array_keys(
            pool,
            tick_lower,
            tick_upper,
            tick_spacing,
            derive_tick_array_key(pool, lower_start),
            derive_tick_array_key(pool, upper_start),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn tick_array_key_rejects_wrong_keys() {
        let pool = Pubkey::new_unique();
        let err = validate_open_position_tick_array_keys(
            pool,
            -1200,
            600,
            60,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        )
        .unwrap_err();
        assert_eq!(err, LpDepositError::InvalidRemainingAccounts.into());
    }
}
```

- [ ] **Step 6: 运行单元测试，确认全部通过**

```bash
cargo test --package lp_handler --lib deposit_without_swap 2>&1 | tail -15
```

预期输出：所有 7 个测试均 `ok`，`0 failed`

---

## Task 4: 实现 open_position CPI 函数

**Files:**
- Modify: `programs/lp_handler/src/instructions/deposit_without_swap.rs`

在 `validate_open_position_tick_array_keys` 之后、`pub fn deposit_without_swap` 之前插入：

- [ ] **Step 1: 插入 open_position_with_token22_nft 私有函数**

```rust
fn open_position_with_token22_nft<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, DepositWithoutSwap<'info>>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    tick_array_lower_start_index: i32,
    tick_array_upper_start_index: i32,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
    action_remaining: &[AccountInfo<'info>],
) -> Result<()> {
    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();
    let accounts = &ctx.accounts;

    let cpi_accounts = clmm_accounts::OpenPositionWithToken22Nft {
        payer: accounts.signer.to_account_info(),
        position_nft_owner: accounts.recipient.to_account_info(),
        position_nft_mint: accounts.position_nft_mint.to_account_info(),
        position_nft_account: accounts.position_nft_account.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        token_account_0: accounts.signer_token0_account.to_account_info(),
        token_account_1: accounts.signer_token1_account.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        rent: accounts.rent.to_account_info(),
        system_program: accounts.system_program.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        associated_token_program: accounts.associated_token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts)
        .with_remaining_accounts(action_remaining.to_vec());

    clmm_cpi::open_position_with_token22_nft(
        cpi_ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
        liquidity,
        amount_0_max,
        amount_1_max,
        true,
        base_flag,
    )?;

    Ok(())
}
```

- [ ] **Step 2: 编译确认无错误**

```bash
cargo test --package lp_handler --lib 2>&1 | tail -5
```

预期输出：`test result: ok.` 或 `0 failed`

---

## Task 5: 实现 increase_liquidity CPI 函数

**Files:**
- Modify: `programs/lp_handler/src/instructions/deposit_without_swap.rs`

在 `open_position_with_token22_nft` 之后、`pub fn deposit_without_swap` 之前插入：

- [ ] **Step 1: 插入 increase_liquidity_v2 私有函数**

```rust
fn increase_liquidity_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, DepositWithoutSwap<'info>>,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
    action_remaining: &[AccountInfo<'info>],
) -> Result<()> {
    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();
    let accounts = &ctx.accounts;

    let cpi_accounts = clmm_accounts::IncreaseLiquidityV2 {
        nft_owner: accounts.signer.to_account_info(),
        nft_account: accounts.position_nft_account.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        token_account_0: accounts.signer_token0_account.to_account_info(),
        token_account_1: accounts.signer_token1_account.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts)
        .with_remaining_accounts(action_remaining.to_vec());

    clmm_cpi::increase_liquidity_v2(cpi_ctx, liquidity, amount_0_max, amount_1_max, base_flag)?;

    Ok(())
}
```

- [ ] **Step 2: 编译确认无错误**

```bash
cargo test --package lp_handler --lib 2>&1 | tail -5
```

预期输出：`test result: ok.` 或 `0 failed`

---

## Task 6: 实现指令主体

**Files:**
- Modify: `programs/lp_handler/src/instructions/deposit_without_swap.rs`

用以下完整实现替换现有的空桩函数 `pub fn deposit_without_swap`：

- [ ] **Step 1: 替换 deposit_without_swap 函数体**

```rust
pub fn deposit_without_swap<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, DepositWithoutSwap<'info>>,
    amount_0_in: u64,
    amount_1_in: u64,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity: u128,
    slippage_bps: u16,
    nft_mint: Option<Pubkey>,
) -> Result<()> {
    // 1. 基础校验
    require!(
        tick_lower_index < tick_upper_index,
        LpDepositError::InvalidTickRange
    );
    require!(
        amount_0_in > 0 || amount_1_in > 0,
        LpDepositError::InvalidDepositAmount
    );
    require!(slippage_bps < 5_000, LpDepositError::InvalidSlippage);

    let balance_0_before = ctx.accounts.signer_token0_account.amount;
    let balance_1_before = ctx.accounts.signer_token1_account.amount;

    require!(
        balance_0_before >= amount_0_in,
        LpDepositError::InsufficientBalance
    );
    require!(
        balance_1_before >= amount_1_in,
        LpDepositError::InsufficientBalance
    );

    // 2. 链上重算 liquidity（考虑 transfer fee）
    let (sqrt_price_x64, tick_spacing) = {
        let pool_state = ctx.accounts.pool_state.load()?;
        (pool_state.sqrt_price_x64, pool_state.tick_spacing)
    };

    let sqrt_ratio_a_x64 = get_sqrt_price_at_tick(tick_lower_index)?;
    let sqrt_ratio_b_x64 = get_sqrt_price_at_tick(tick_upper_index)?;

    let amount_0_transfer_fee =
        utils::get_transfer_fee_for_amount(ctx.accounts.vault_0_mint.as_ref(), amount_0_in)?;
    let amount_1_transfer_fee =
        utils::get_transfer_fee_for_amount(ctx.accounts.vault_1_mint.as_ref(), amount_1_in)?;
    let amount_0_post_fee = amount_0_in
        .checked_sub(amount_0_transfer_fee)
        .ok_or(LpDepositError::MathOverflow)?;
    let amount_1_post_fee = amount_1_in
        .checked_sub(amount_1_transfer_fee)
        .ok_or(LpDepositError::MathOverflow)?;

    let computed_liquidity = liquidity_math::get_liquidity_from_amounts(
        sqrt_price_x64,
        sqrt_ratio_a_x64,
        sqrt_ratio_b_x64,
        amount_0_post_fee,
        amount_1_post_fee,
    );

    // 3. liquidity 偏差校验
    validate_liquidity_slippage(computed_liquidity, liquidity, slippage_bps)?;

    // 4. base_flag（与 zap_common 保持一致）
    let base_flag = if amount_0_in == 0 {
        Some(false)
    } else {
        Some(true)
    };

    // 5. 提前取出事件所需字段（避免后续借用冲突）
    let position_nft_mint_for_event = match nft_mint {
        None => ctx.accounts.position_nft_mint.key(),
        Some(mint) => mint,
    };

    // 6. 分支：开仓 vs 追加
    if nft_mint.is_none() {
        // ---- 开仓 ----
        require!(
            ctx.accounts.position_nft_mint.is_signer,
            LpDepositError::InvalidPositionNftAccount
        );

        validate_open_position_tick_array_keys(
            ctx.accounts.pool_state.key(),
            tick_lower_index,
            tick_upper_index,
            tick_spacing,
            ctx.accounts.tick_array_lower.key(),
            ctx.accounts.tick_array_upper.key(),
        )?;

        let tick_array_lower_start_index =
            TickArrayState::get_array_start_index(tick_lower_index, tick_spacing);
        let tick_array_upper_start_index =
            TickArrayState::get_array_start_index(tick_upper_index, tick_spacing);

        open_position_with_token22_nft(
            &ctx,
            tick_lower_index,
            tick_upper_index,
            tick_array_lower_start_index,
            tick_array_upper_start_index,
            computed_liquidity,
            amount_0_in,
            amount_1_in,
            base_flag,
            ctx.remaining_accounts,
        )?;
    } else {
        // ---- 追加 ----
        let nft_mint_key = nft_mint.unwrap();

        validate_position_nft_account(
            &ctx.accounts.position_nft_account,
            nft_mint_key,
            ctx.accounts.signer.key(),
        )?;

        validate_personal_position(
            &ctx.accounts.personal_position,
            ctx.accounts.pool_state.key(),
            tick_lower_index,
            tick_upper_index,
        )?;

        increase_liquidity_v2(
            &ctx,
            computed_liquidity,
            amount_0_in,
            amount_1_in,
            base_flag,
            ctx.remaining_accounts,
        )?;
    }

    // 7. reload 余额，计算实际消耗和剩余
    ctx.accounts.signer_token0_account.reload()?;
    ctx.accounts.signer_token1_account.reload()?;

    let balance_0_after_cpi = ctx.accounts.signer_token0_account.amount;
    let balance_1_after_cpi = ctx.accounts.signer_token1_account.amount;

    let spent_0 = balance_0_before
        .checked_sub(balance_0_after_cpi)
        .unwrap_or_else(|| {
            msg!(
                "WARN: spent_0 underflow: pre={}, after={}",
                balance_0_before,
                balance_0_after_cpi
            );
            0
        });
    let spent_1 = balance_1_before
        .checked_sub(balance_1_after_cpi)
        .unwrap_or_else(|| {
            msg!(
                "WARN: spent_1 underflow: pre={}, after={}",
                balance_1_before,
                balance_1_after_cpi
            );
            0
        });
    let leftover_0 = amount_0_in.checked_sub(spent_0).unwrap_or_else(|| {
        msg!(
            "WARN: leftover_0 underflow: max={}, spent={}",
            amount_0_in,
            spent_0
        );
        0
    });
    let leftover_1 = amount_1_in.checked_sub(spent_1).unwrap_or_else(|| {
        msg!(
            "WARN: leftover_1 underflow: max={}, spent={}",
            amount_1_in,
            spent_1
        );
        0
    });

    // 8. 发出事件
    emit!(LpHandlerIncreaseLiquidityEvent {
        pool: ctx.accounts.pool_state.key(),
        position_nft_mint: position_nft_mint_for_event,
        principal_0: spent_0,
        principal_1: spent_1,
        token0_mint: ctx.accounts.vault_0_mint.key(),
        token1_mint: ctx.accounts.vault_1_mint.key(),
        tick_lower_index,
        tick_upper_index,
        liquidity: computed_liquidity,
        amount_0_in,
        amount_1_in,
        return_amount_0: leftover_0,
        return_amount_1: leftover_1,
    });

    // 9. 退款：将剩余 token 转到 refund accounts（若与 signer accounts 不同）
    let user_ai = ctx.accounts.signer.to_account_info();
    let recipient_ai = ctx.accounts.recipient.to_account_info();
    let user_token0_ai = ctx.accounts.signer_token0_account.to_account_info();
    let user_token1_ai = ctx.accounts.signer_token1_account.to_account_info();
    let refund_token0_ai = ctx.accounts.refund_token0_account.to_account_info();
    let refund_token1_ai = ctx.accounts.refund_token1_account.to_account_info();
    let vault_0_mint_ai = ctx.accounts.vault_0_mint.to_account_info();
    let vault_1_mint_ai = ctx.accounts.vault_1_mint.to_account_info();
    let vault_0_mint_decimals = ctx.accounts.vault_0_mint.decimals;
    let vault_1_mint_decimals = ctx.accounts.vault_1_mint.decimals;
    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let token_program_2022_ai = ctx.accounts.token_program_2022.to_account_info();

    if !crate::consts::is_native_sol_mint(&ctx.accounts.vault_0_mint.key())
        && refund_token0_ai.key() != user_token0_ai.key()
    {
        zap_common::transfer_token_to_fee_accounts(
            user_ai.clone(),
            user_token0_ai.clone(),
            refund_token0_ai,
            vault_0_mint_ai,
            vault_0_mint_decimals,
            token_program_ai.clone(),
            Some(token_program_2022_ai.clone()),
            leftover_0,
        )?;
    }

    if !crate::consts::is_native_sol_mint(&ctx.accounts.vault_1_mint.key())
        && refund_token1_ai.key() != user_token1_ai.key()
    {
        zap_common::transfer_token_to_fee_accounts(
            user_ai.clone(),
            user_token1_ai.clone(),
            refund_token1_ai,
            vault_1_mint_ai,
            vault_1_mint_decimals,
            token_program_ai.clone(),
            Some(token_program_2022_ai.clone()),
            leftover_1,
        )?;
    }

    // 10. wSOL unwrap：close wSOL token account，lamports 发往 recipient（或 signer）
    let wsol_dest = |signer: Pubkey, recipient: Pubkey| -> AccountInfo<'info> {
        if signer == recipient {
            user_ai.clone()
        } else {
            recipient_ai.clone()
        }
    };

    if crate::consts::is_native_sol_mint(&ctx.accounts.vault_0_mint.key()) {
        zap_common::unwrap_wsol_to_destination(
            user_ai.clone(),
            user_token0_ai.clone(),
            wsol_dest(user_ai.key(), recipient_ai.key()),
            token_program_ai.clone(),
            Some(token_program_2022_ai.clone()),
        )?;
    }

    if crate::consts::is_native_sol_mint(&ctx.accounts.vault_1_mint.key()) {
        zap_common::unwrap_wsol_to_destination(
            user_ai.clone(),
            user_token1_ai.clone(),
            wsol_dest(user_ai.key(), recipient_ai.key()),
            token_program_ai.clone(),
            Some(token_program_2022_ai),
        )?;
    }

    Ok(())
}
```

- [ ] **Step 2: 运行单元测试确认全部通过**

```bash
cargo test --package lp_handler --lib deposit_without_swap 2>&1 | tail -15
```

预期：所有测试均 `ok`，`0 failed`

---

## Task 7: 在 lib.rs 注册入口

**Files:**
- Modify: `programs/lp_handler/src/lib.rs`

- [ ] **Step 1: 在 lib.rs 的 `#[program]` 块中追加入口函数**

在 `close_security_config` 函数之后、最后一个 `}` 之前插入：

```rust
    /// 双币直接投入（开仓或追加），不执行任何 swap，剩余双币原样退回。
    ///
    /// - `nft_mint` 为 None：开仓（open_position_with_token22_nft）
    /// - `nft_mint` 为 Some(mint)：追加（increase_liquidity_v2）
    pub fn deposit_without_swap<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, DepositWithoutSwap<'info>>,
        amount_0_in: u64,
        amount_1_in: u64,
        tick_lower_index: i32,
        tick_upper_index: i32,
        liquidity: u128,
        slippage_bps: u16,
        nft_mint: Option<Pubkey>,
    ) -> Result<()> {
        let signer = ctx.accounts.signer.key();
        let recipient = ctx.accounts.recipient.key();
        let fee_owner = ctx.accounts.fee_owner.key();
        secure_entrypoint!(
            ctx,
            signer = signer,
            recipient = recipient,
            fee_owner = fee_owner,
            pool_state = &ctx.accounts.pool_state,
            body = instructions::deposit_without_swap(
                ctx,
                amount_0_in,
                amount_1_in,
                tick_lower_index,
                tick_upper_index,
                liquidity,
                slippage_bps,
                nft_mint,
            )
        )
    }
```

- [ ] **Step 2: 编译确认无错误**

```bash
cargo test --package lp_handler --lib 2>&1 | tail -5
```

预期输出：`test result: ok.` 或 `0 failed`

---

## Task 8: 全量编译验证 + 运行现有测试

**Files:** 无新增文件

- [ ] **Step 1: 运行全部 Rust 单元测试**

```bash
cargo test --package lp_handler --lib 2>&1 | tail -20
```

预期：所有已有测试 + 新增 7 个测试均通过，`0 failed`

- [ ] **Step 2: anchor build 编译整个项目**

```bash
anchor build 2>&1 | tail -20
```

预期：`Finished release [optimized] target(s)`，无 error

- [ ] **Step 3: 确认 IDL 包含新指令**

```bash
cat target/idl/lp_handler.json | grep -A 2 "deposit_without_swap"
```

预期：输出中出现 `"deposit_without_swap"` 及其参数字段

---

## 规范自查（Spec Self-Review）

| 设计要求 | 覆盖 Task |
|----------|-----------|
| 单一指令，nft_mint 区分开仓/追加 | Task 6 |
| 完全不做 swap（主配平 + cleanup 均跳过） | Task 6（无 swap 调用） |
| 链上重算 liquidity + slippage 校验 | Task 3, Task 6 |
| 无 return_mint，剩余双币原样退回 | Task 6 step 1（退款逻辑） |
| remaining_accounts 直接作为 action_remaining | Task 6（ctx.remaining_accounts 直传） |
| recipient 逻辑与现有一致 | Task 6（refund、wSOL、open_position_nft_owner） |
| 安全层 secure_entrypoint! 包装 | Task 7 |
| 事件字段与现有一致 | Task 6 step 1 |
| 新增 LiquiditySlippageExceeded 错误码 | Task 1 |
| 不改动其他指令文件 | 全程 |
