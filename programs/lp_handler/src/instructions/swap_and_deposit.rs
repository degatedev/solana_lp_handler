use anchor_lang::prelude::*;
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;

use crate::{utils, IncreaseLiquidityEvent, LpDepositError, SwapExecutedEvent};

use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{AmmConfig, ObservationState, PoolState, TickArrayState};

/// swap_and_deposit 所需的所有账户
/// 包含 swap_v2 和 open_position_v2 的全部账户（有些可以复用，比如 pool_state、token_program 等）
#[derive(Accounts)]
#[instruction(
    deposit_amount: u64,
    deposit_mint: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity:i128,
    slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
)]
pub struct SwapAndDeposit<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program (主网: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK)
    /// CHECK: 前端传入 Raydium CLMM programId
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// 支付者 / 签名者
    #[account(mut)]
    pub user: Signer<'info>,

    /// AMM 配置账户（swap 和 position 都需要通过 pool_state 关联）
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool 状态账户（swap 和 open_position 都需要）
    #[account(
        mut,
        constraint = pool_state.load()?.amm_config == amm_config.key()
    )]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Observation 状态（swap 需要）
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    #[account(
        mut,
        token::mint = token_vault_0.mint,
    )]
    pub user_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint,
    )]
    pub user_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: Receives the position NFT
    #[account(address = user.key())]
    pub position_nft_owner: UncheckedAccount<'info>,

    /// Unique token mint address, initialize in contract
    #[account(mut)]
    pub position_nft_mint: Signer<'info>,

    /// CHECK: ATA address where position NFT will be minted, initialize in contract
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: Deprecated: protocol_position is deprecated and kept for compatibility.
    pub protocol_position: UncheckedAccount<'info>,

    /// CHECK:  Account to store data for the position's lower tick
    #[account(mut, constraint = tick_array_lower.load()?.pool_id == pool_state.key())]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// Stores init state for the upper tick
    #[account(mut, constraint = tick_array_upper.load()?.pool_id == pool_state.key())]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    pub memo_program: Program<'info, Memo>,

    /// CHECK: Personal position state account, validated by Raydium CLMM program
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

    /// The address that holds pool tokens for token_0
    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The address that holds pool tokens for token_1
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Sysvar for token mint and ATA creation
    pub rent: Sysvar<'info, Rent>,

    /// Program to create the position manager state account
    pub system_program: Program<'info, System>,

    /// Program to transfer for token account
    pub token_program: Program<'info, Token>,

    /// Program to create an ATA for receiving position NFT
    pub associated_token_program: Program<'info, AssociatedToken>,

    /// Program to create NFT mint/token account and transfer for token22 account
    pub token_program_2022: Program<'info, Token2022>,

    /// The mint of token vault 0
    #[account(
        address = token_vault_0.mint
    )]
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of token vault 1
    #[account(
        address = token_vault_1.mint
    )]
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,
    // ======== IMPORTANT: remaining accounts for swap_v2 =========
    // MUST BE: [bitmap_extension?] + [swap tick arrays ONLY]
    // open_position tick arrays MUST NOT be here!
    //
    // 所有 swap_v2 tick arrays 都在这里动态提供（前端传入）
    //
    // 例如：
    // [
    //   bitmap_extension?,
    //   swap_tick_array_0,
    //   swap_tick_array_1,
    //   swap_tick_array_2,
    // ]
    //
    // open_position 的 tick_array_lower / upper 由上面两个字段指定，不混在这里
    //
}

/// 先调用 Raydium swap_v2 换币，再调用 open_position_v2 开仓添加流动性
/// 使用 Raydium 的 liquidity_math 精确计算最优 swap 比例
pub fn swap_and_deposit<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapAndDeposit<'info>>,
    deposit_amount: u64,
    deposit_mint: Pubkey,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity: i128,
    slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
) -> Result<()> {
    let timestamp = Clock::get()?.unix_timestamp;
    // 防钓鱼：仓位 NFT 只能铸给交易签名者本人
    require_keys_eq!(
        ctx.accounts.position_nft_owner.key(),
        ctx.accounts.user.key(),
        LpDepositError::InvalidPositionNftOwner
    );
    // tick 区间必须合法
    require!(
        tick_lower_index < tick_upper_index,
        LpDepositError::InvalidTickRange
    );
    // 校验 position_nft_account 必须是 (position_nft_owner, position_nft_mint, Token2022) 的 ATA 地址
    // 注意：该 ATA 可能尚未初始化（由下游 CPI 创建），因此只校验地址本身，不校验 owner/program。
    let expected_position_nft_ata = utils::derive_ata_address(
        &ctx.accounts.position_nft_owner.key(),
        &ctx.accounts.position_nft_mint.key(),
        &ctx.accounts.token_program_2022.key(),
        &ctx.accounts.associated_token_program.key(),
    );
    require_keys_eq!(
        ctx.accounts.position_nft_account.key(),
        expected_position_nft_ata,
        LpDepositError::InvalidPositionNftAccount
    );

    // 1. 校验存入的 mint 是否为池子的 token0 或 token1
    require!(
        deposit_mint == ctx.accounts.vault_0_mint.key()
            || deposit_mint == ctx.accounts.vault_1_mint.key(),
        LpDepositError::InvalidDepositMint
    );
    require!(slippage_bps < 5_000, LpDepositError::InvalidSlippage);

    // 2. 判断存入的是 token0 还是 token1
    let is_token0 = deposit_mint == ctx.accounts.vault_0_mint.key();

    // 3. 读取当前池子状态并计算 swap 数量
    // 注意：需要在 swap 之前保存 tick_spacing，因为 swap 会修改 pool_state
    let mut swap_amount_min = 0;
    let (_current_tick, swap_amount_in, swap_amount_out, tick_spacing, current_sqrt_price_x64) = {
        let pool_state = ctx.accounts.pool_state.load()?;
        let current_sqrt_price_x64 = pool_state.sqrt_price_x64;
        let current_tick = pool_state.tick_current;
        let tick_spacing = pool_state.tick_spacing;

        // 使用 Raydium liquidity_math 计算最优 swap 数量
        let (swap_amount, swap_amount_out) = utils::calculate_optimal_swap_amount(
            deposit_amount,
            is_token0,
            current_tick,
            tick_lower_index,
            tick_upper_index,
            current_sqrt_price_x64,
            liquidity,
        )?;
        (
            current_tick,
            swap_amount,
            swap_amount_out,
            tick_spacing,
            current_sqrt_price_x64,
        )
    };

    // 4. 记录 swap 前的余额
    let balance_0_before = ctx.accounts.user_token0_account.amount;
    let balance_1_before = ctx.accounts.user_token1_account.amount;

    // 5. 执行 swap（如果需要）
    if swap_amount_in > 0 {
        // 计算最小输出（滑点保护）
        let mut min_amount_out = utils::calc_min_amount_out(
            swap_amount_in,
            is_token0,
            current_sqrt_price_x64,
            slippage_bps,
        )?;
        swap_amount_min = swap_amount_in;
        if swap_amount_in != deposit_amount {
            swap_amount_min = utils::apply_slippage_bps_floor(swap_amount_in, slippage_bps)?;
            min_amount_out = utils::calc_min_amount_out(
                swap_amount_min,
                is_token0,
                current_sqrt_price_x64,
                slippage_bps,
            )?;
        }
        msg!(
            "Swap params: swap_amount={},min_amount_out={},swap_amount_out={}",
            swap_amount_min,
            min_amount_out,
            swap_amount_out
        );
        // 执行 swap
        swap_v2(&ctx, swap_amount_min, min_amount_out, 0, is_token0)?;
        // 6. 读取 swap 后的余额并计算实际可用数量
        ctx.accounts.user_token0_account.reload()?;
        ctx.accounts.user_token1_account.reload()?;
        let amount_out_after = if is_token0 {
            ctx.accounts
                .user_token1_account
                .amount
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?
        } else {
            ctx.accounts
                .user_token0_account
                .amount
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?
        };
        // 发出 Swap 事件
        emit!(SwapExecutedEvent {
            timestamp,
            user: ctx.accounts.user.key(),
            pool: ctx.accounts.pool_state.key(),
            amount_in: swap_amount_min,
            amount_out: amount_out_after,
            amount_out_min: min_amount_out,
            is_token0_input: is_token0,
            token0_mint: ctx.accounts.vault_0_mint.key(),
            token1_mint: ctx.accounts.vault_1_mint.key(),
            slippage_bps,
        });
    }

    let balance_0_after = ctx.accounts.user_token0_account.amount;
    let balance_1_after = ctx.accounts.user_token1_account.amount;

    // 计算本次操作实际可用的代币数量
    // - 存入的代币：deposit_amount - swap_amount（剩余部分）
    // - swap 得到的代币：余额增加量
    let (amount_0_max, amount_1_max) = if is_token0 {
        (
            deposit_amount
                .checked_sub(swap_amount_min)
                .ok_or(LpDepositError::MathOverflow)?,
            balance_1_after
                .checked_sub(balance_1_before)
                .ok_or(LpDepositError::MathOverflow)?,
        )
    } else {
        (
            balance_0_after
                .checked_sub(balance_0_before)
                .ok_or(LpDepositError::MathOverflow)?,
            deposit_amount
                .checked_sub(swap_amount_min)
                .ok_or(LpDepositError::MathOverflow)?,
        )
    };

    let tick_array_lower_start_index =
        TickArrayState::get_array_start_index(tick_lower_index, tick_spacing as u16);
    let tick_array_upper_start_index =
        TickArrayState::get_array_start_index(tick_upper_index, tick_spacing as u16);
    let base_flag = if amount_0_max == 0 {
        Some(false)
    } else if amount_1_max == 0 {
        Some(true)
    } else if is_token0 {
        Some(false)
    } else {
        Some(true)
    };

    msg!(
        "open_position for LP: amount_0_max={}, amount_1_max={} (deposit_amount={}, swap_amount_in={}, swap_amount_out={}, base_flag={})",
        amount_0_max,
        amount_1_max,
        deposit_amount,
        swap_amount_in,
        swap_amount_out,
        base_flag.unwrap()
    );

    // 7. 添加流动性
    // 注意：这里固定传 0 是“刻意设计”
    // - `liquidity` 参数仅用于上面的 `calculate_optimal_swap_amount` 计算最优兑换比例
    // - 开仓/加流动性时让 Raydium 根据 amount_0_max/amount_1_max 自动计算实际 liquidity
    open_position_with_token22_nft(
        &ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
        0,
        amount_0_max,
        amount_1_max,
        base_flag, // base_flag
    )?;

    // 销毁ata账户

    // 发出流动性添加事件
    emit!(IncreaseLiquidityEvent {
        timestamp,
        user: ctx.accounts.user.key(),
        pool: ctx.accounts.pool_state.key(),
        position_nft_mint: ctx.accounts.position_nft_mint.key(),
        amount_0: amount_0_max,
        amount_1: amount_1_max,
        token0_mint: ctx.accounts.vault_0_mint.key(),
        token1_mint: ctx.accounts.vault_1_mint.key(),
        tick_lower_index,
        tick_upper_index,
        liquidity,
    });

    Ok(())
}

fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, SwapAndDeposit<'info>>,
    swap_amount: u64,
    swap_other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_token0: bool,
) -> Result<()> {
    // 使用解构简化代码
    let accounts = &ctx.accounts;
    let cpi_program = accounts.raydium_clmm_program.to_account_info();

    // 根据 is_token0 选择对应的账户（一次性解构）
    let (input_token, output_token, input_vault, output_vault, input_mint, output_mint) =
        match is_token0 {
            true => (
                &accounts.user_token0_account,
                &accounts.user_token1_account,
                &accounts.token_vault_0,
                &accounts.token_vault_1,
                &accounts.vault_0_mint,
                &accounts.vault_1_mint,
            ),
            false => (
                &accounts.user_token1_account,
                &accounts.user_token0_account,
                &accounts.token_vault_1,
                &accounts.token_vault_0,
                &accounts.vault_1_mint,
                &accounts.vault_0_mint,
            ),
        };

    let cpi_accounts = clmm_accounts::SwapSingleV2 {
        payer: accounts.user.to_account_info(),
        amm_config: accounts.amm_config.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        observation_state: accounts.observation_state.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        memo_program: accounts.memo_program.to_account_info(),
        input_token_account: input_token.to_account_info(),
        output_token_account: output_token.to_account_info(),
        input_vault: input_vault.to_account_info(),
        output_vault: output_vault.to_account_info(),
        input_vault_mint: input_mint.to_account_info(),
        output_vault_mint: output_mint.to_account_info(),
    };
    let swap_remaining = ctx.remaining_accounts.to_vec();
    // 轻量校验 remaining_accounts：限制数量并要求 owner 为 Raydium CLMM program（tick array/bitmap 等应满足）
    require!(
        swap_remaining.len() <= 32,
        LpDepositError::InvalidRemainingAccounts
    );
    for acc in swap_remaining.iter() {
        require_keys_eq!(
            *acc.owner,
            accounts.raydium_clmm_program.key(),
            LpDepositError::InvalidRemainingAccounts
        );
    }

    let cpi_ctx =
        CpiContext::new(cpi_program.clone(), cpi_accounts).with_remaining_accounts(swap_remaining);

    clmm_cpi::swap_v2(
        cpi_ctx,
        swap_amount,
        swap_other_amount_threshold,
        sqrt_price_limit_x64,
        true,
    )?;

    Ok(())
}

fn open_position_with_token22_nft<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, SwapAndDeposit<'info>>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    tick_array_lower_start_index: i32,
    tick_array_upper_start_index: i32,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
) -> Result<()> {
    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();

    // 使用解构简化代码
    let accounts = &ctx.accounts;
    let cpi_accounts = clmm_accounts::OpenPositionWithToken22Nft {
        payer: accounts.user.to_account_info(),
        position_nft_owner: accounts.position_nft_owner.to_account_info(),
        position_nft_mint: accounts.position_nft_mint.to_account_info(),
        position_nft_account: accounts.position_nft_account.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        token_account_0: accounts.user_token0_account.to_account_info(),
        token_account_1: accounts.user_token1_account.to_account_info(),
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

    let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);

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
