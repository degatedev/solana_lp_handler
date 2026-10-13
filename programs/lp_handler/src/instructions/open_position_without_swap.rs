use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::program::AmmV3;
use raydium_amm_v3::states::{PoolState, TickArrayState, TICK_ARRAY_SEED};

use crate::instructions::zap_common::{
    compute_and_validate_liquidity, settle_deposit_without_swap, split_remaining_accounts,
    WithoutSwapEventParams, WithoutSwapSettleAccounts,
};
use crate::{utils, LpDepositError, SECURITY_CONFIG_SEED};

#[derive(Accounts)]
#[instruction(
    amount_0_in: u64,
    amount_1_in: u64,
    tick_lower_index: i32,
    tick_upper_index: i32,
    liquidity: u128,
    slippage_bps: u16,
)]
pub struct OpenPositionWithoutSwap<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program
    #[account(address = raydium_amm_v3::ID)]
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// CHECK: position NFT 的接收者（owner）。安全层校验其 authority 关系。必须是第二个账户。
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

    // ========== Position 相关账户 ==========

    /// position NFT 的 mint（新 keypair，必须签名；CPI 会创建并 mint）
    #[account(mut)]
    pub position_nft_mint: Signer<'info>,

    /// CHECK: position NFT 将被 mint 到的 token account（通常是 ATA）。
    /// 本程序不校验其派生规则，依赖 Raydium open_position CPI 在内部验证 mint/owner 一致性。
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: protocol_position 已废弃，仅为兼容保留
    pub protocol_position: UncheckedAccount<'info>,

    /// CHECK: 允许传入尚未初始化的 PersonalPosition PDA；由 Raydium open_position CPI 初始化。
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

    /// CHECK: 允许传入尚未初始化的 TickArray PDA；地址在本程序内按 Raydium 规则校验。
    #[account(mut)]
    pub tick_array_lower: UncheckedAccount<'info>,

    /// CHECK: 允许传入尚未初始化的 TickArray PDA；地址在本程序内按 Raydium 规则校验。
    #[account(mut)]
    pub tick_array_upper: UncheckedAccount<'info>,
    // ======== remaining_accounts =========
    // 格式：[separator: program_id] [optional: tick_array_bitmap_extension] [action_tick_arrays...]
    // 安全层要求 remaining_accounts 中恰好出现 1 次分隔符（program_id）。
    // 链上调用 split_remaining_accounts 丢弃分隔符前后的空段，只把分隔符后的 action 部分传给 Raydium CPI。
}

// ========== 辅助函数 ==========

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
    let lower_start = TickArrayState::get_array_start_index(tick_lower_index, tick_spacing);
    let upper_start = TickArrayState::get_array_start_index(tick_upper_index, tick_spacing);

    require_keys_eq!(
        tick_array_lower,
        derive_tick_array_key(pool_state, lower_start),
        LpDepositError::InvalidRemainingAccounts
    );
    require_keys_eq!(
        tick_array_upper,
        derive_tick_array_key(pool_state, upper_start),
        LpDepositError::InvalidRemainingAccounts
    );
    Ok(())
}

fn open_position_cpi<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, OpenPositionWithoutSwap<'info>>,
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

// ========== 指令主体 ==========

pub fn open_position_without_swap<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, OpenPositionWithoutSwap<'info>>,
    amount_0_in: u64,
    amount_1_in: u64,
    tick_lower_index: i32,
    tick_upper_index: i32,
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

    let (computed_liquidity, tick_spacing) = compute_and_validate_liquidity(
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

    validate_open_position_tick_array_keys(
        ctx.accounts.pool_state.key(),
        tick_lower_index,
        tick_upper_index,
        tick_spacing,
        ctx.accounts.tick_array_lower.key(),
        ctx.accounts.tick_array_upper.key(),
    )?;

    let base_flag = if amount_0_in == 0 {
        Some(false)
    } else {
        Some(true)
    };

    let position_nft_mint = ctx.accounts.position_nft_mint.key();

    let tick_array_lower_start_index =
        TickArrayState::get_array_start_index(tick_lower_index, tick_spacing);
    let tick_array_upper_start_index =
        TickArrayState::get_array_start_index(tick_upper_index, tick_spacing);

    let (_, action_remaining) = split_remaining_accounts(ctx.remaining_accounts)?;

    open_position_cpi(
        &ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_array_key_derivation_matches_raydium_formula() {
        let pool = Pubkey::new_unique();
        let tick_spacing = 60u16;
        let tick_lower = -1200i32;
        let tick_upper = 600i32;

        let lower_start = TickArrayState::get_array_start_index(tick_lower, tick_spacing);
        let upper_start = TickArrayState::get_array_start_index(tick_upper, tick_spacing);

        assert!(validate_open_position_tick_array_keys(
            pool,
            tick_lower,
            tick_upper,
            tick_spacing,
            derive_tick_array_key(pool, lower_start),
            derive_tick_array_key(pool, upper_start),
        )
        .is_ok());
    }

    #[test]
    fn tick_array_key_rejects_wrong_keys() {
        let pool = Pubkey::new_unique();
        let err = validate_open_position_tick_array_keys(
            pool, -1200, 600, 60,
            Pubkey::new_unique(), Pubkey::new_unique(),
        )
        .unwrap_err();
        assert_eq!(err, LpDepositError::InvalidRemainingAccounts.into());
    }
}
