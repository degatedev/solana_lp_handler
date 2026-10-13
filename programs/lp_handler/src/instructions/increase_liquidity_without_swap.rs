use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{PersonalPositionState, PoolState, TickArrayState};

use crate::instructions::zap_common::{
    compute_and_validate_liquidity, settle_deposit_without_swap, split_remaining_accounts,
    WithoutSwapEventParams, WithoutSwapSettleAccounts,
};
use crate::{utils, LpDepositError, SECURITY_CONFIG_SEED};

#[derive(Accounts)]
#[instruction(
    amount_0_in: u64,
    amount_1_in: u64,
    liquidity: u128,
    slippage_bps: u16,
)]
pub struct IncreaseLiquidityWithoutSwap<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// CHECK: withdraw 和 claim 的接收者。必须是第二个账户，安全层依赖位置。
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

    /// refund 账户的 authority 在运行时校验为 recipient 或 signer（支持硬件钱包场景）
    #[account(
        mut,
        token::mint = token_vault_0.mint,
    )]
    pub refund_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// refund 账户的 authority 在运行时校验为 recipient 或 signer（支持硬件钱包场景）
    #[account(
        mut,
        token::mint = token_vault_1.mint,
    )]
    pub refund_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Pool 状态
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// 安全层白名单校验用；本指令不收取手续费，无需 mut。
    pub fee_owner: SystemAccount<'info>,

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

    // ========== Position 相关账户（类型安全约束）==========

    /// 持有 position NFT 的 token account（amount=1, authority=signer）
    #[account(
        constraint = position_nft_account.mint == personal_position.nft_mint,
        constraint = position_nft_account.amount == 1,
        token::authority = signer
    )]
    pub position_nft_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// 为该 position 增加流动性
    #[account(mut, constraint = personal_position.pool_id == pool_state.key())]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// CHECK: protocol_position 已废弃，仅为兼容保留
    pub protocol_position: UncheckedAccount<'info>,

    /// lower tick 对应的 TickArray（必须已初始化）
    #[account(mut, constraint = tick_array_lower.load()?.pool_id == pool_state.key())]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// upper tick 对应的 TickArray（必须已初始化）
    #[account(mut, constraint = tick_array_upper.load()?.pool_id == pool_state.key())]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,
    // ======== remaining_accounts =========
    // 格式：[separator: program_id] [optional: tick_array_bitmap_extension] [action_tick_arrays...]
    // 安全层要求 remaining_accounts 中恰好出现 1 次分隔符（program_id）。
    // 链上调用 split_remaining_accounts 丢弃分隔符前后的空段，只把分隔符后的 action 部分传给 Raydium CPI。
}

// ========== 辅助函数 ==========

fn increase_liquidity_cpi<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, IncreaseLiquidityWithoutSwap<'info>>,
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

// ========== 指令主体 ==========

pub fn increase_liquidity_without_swap<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, IncreaseLiquidityWithoutSwap<'info>>,
    amount_0_in: u64,
    amount_1_in: u64,
    liquidity: u128,
    slippage_bps: u16,
) -> Result<()> {
    utils::validate_refund_authorities(
        ctx.accounts.signer.key(),
        ctx.accounts.recipient.key(),
        ctx.accounts.refund_token0_account.owner,
        ctx.accounts.refund_token1_account.owner,
    )?;

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

    // tick 范围直接从 personal_position 读取，无需参数传入
    let tick_lower_index = ctx.accounts.personal_position.tick_lower_index;
    let tick_upper_index = ctx.accounts.personal_position.tick_upper_index;

    let (computed_liquidity, _) = compute_and_validate_liquidity(
        &ctx.accounts.pool_state,
        tick_lower_index,
        tick_upper_index,
        ctx.accounts.vault_0_mint.as_ref(),
        ctx.accounts.vault_1_mint.as_ref(),
        amount_0_in,
        amount_1_in,
        liquidity,
        slippage_bps,
    )?;

    let base_flag = if amount_0_in == 0 {
        Some(false)
    } else {
        Some(true)
    };

    let position_nft_mint = ctx.accounts.position_nft_account.mint;

    let (_, action_remaining) = split_remaining_accounts(ctx.remaining_accounts)?;

    increase_liquidity_cpi(
        &ctx,
        computed_liquidity,
        amount_0_in,
        amount_1_in,
        base_flag,
        action_remaining,
    )?;

    ctx.accounts.signer_token0_account.reload()?;
    ctx.accounts.signer_token1_account.reload()?;

    let balance_0_after = ctx.accounts.signer_token0_account.amount;
    let balance_1_after = ctx.accounts.signer_token1_account.amount;

    settle_deposit_without_swap(
        WithoutSwapSettleAccounts {
            user: ctx.accounts.signer.to_account_info(),
            recipient: ctx.accounts.recipient.to_account_info(),
            user_token0: ctx.accounts.signer_token0_account.to_account_info(),
            user_token1: ctx.accounts.signer_token1_account.to_account_info(),
            refund_token0: ctx.accounts.refund_token0_account.to_account_info(),
            refund_token1: ctx.accounts.refund_token1_account.to_account_info(),
            vault_0_mint: ctx.accounts.vault_0_mint.to_account_info(),
            vault_1_mint: ctx.accounts.vault_1_mint.to_account_info(),
            vault_0_mint_decimals: ctx.accounts.vault_0_mint.decimals,
            vault_1_mint_decimals: ctx.accounts.vault_1_mint.decimals,
            token_program: ctx.accounts.token_program.to_account_info(),
            token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        },
        WithoutSwapEventParams {
            pool: ctx.accounts.pool_state.key(),
            position_nft_mint,
            token0_mint: ctx.accounts.vault_0_mint.key(),
            token1_mint: ctx.accounts.vault_1_mint.key(),
            tick_lower_index,
            tick_upper_index,
            computed_liquidity,
            amount_0_in,
            amount_1_in,
        },
        balance_0_before,
        balance_0_after,
        balance_1_before,
        balance_1_after,
    )?;

    Ok(())
}
