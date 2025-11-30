use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::memo::Memo;
use anchor_spl::token::{self, Token};
use anchor_spl::token_2022::{self, Token2022};
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_amm_v3::program::AmmV3;

use crate::{utils, DecreaseLiquidityEvent, LpDepositError};
use raydium_amm_v3::cpi as clmm_cpi;
use raydium_amm_v3::cpi::accounts as clmm_accounts;
use raydium_amm_v3::states::{
    AmmConfig, ObservationState, PersonalPositionState, PoolState, TickArrayState,
};

#[derive(Accounts)]
#[instruction(
  liquidity:u128,
  mint_amount_0:u64,
  mint_amount_1:u64,
  swap_to_token_mint: Pubkey,
  slippage_bps: u16, // 滑点，单位为基点 (1 bps = 0.01%)
  fee_percent: u16,
  convert_to_usdc: bool,
)]
pub struct DecreaseLiquidity<'info> {
    // ========== 公共账户 ==========
    /// Raydium CLMM program (主网: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK)
    /// CHECK: 前端传入 Raydium CLMM programId
    pub raydium_clmm_program: Program<'info, AmmV3>,

    /// 支付者 / 签名者
    #[account(mut)]
    pub user: Signer<'info>,

    /// user 的 token0 ATA 账户（如果不存在则自动创建）
    #[account(
          init_if_needed,
          payer = user,
          associated_token::mint = vault_0_mint,
          associated_token::authority = user,
      )]
    pub user_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// user 的 token1 ATA 账户（如果不存在则自动创建）
    #[account(
          init_if_needed,
          payer = user,
          associated_token::mint = vault_1_mint,
          associated_token::authority = user,
      )]
    pub user_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub fee_token0_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub fee_token1_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// AMM 配置账户（swap 和 position 都需要通过 pool_state 关联）
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool 状态账户（swap 和 open_position 都需要）
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Observation 状态（swap 需要）
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

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

    pub memo_program: Program<'info, Memo>,

    /// The address that holds pool tokens for token_0
    #[account(mut)]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The address that holds pool tokens for token_1
    #[account(mut)]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

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

    /// CHECK: Receives the position NFT
    pub position_nft_owner: UncheckedAccount<'info>,

    /// Unique token mint address, initialize in contract

    /// CHECK: ATA address where position NFT will be minted, initialize in contract
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: Deprecated: protocol_position is deprecated and kept for compatibility.
    pub protocol_position: UncheckedAccount<'info>,

    /// CHECK: Personal position state account, validated by Raydium CLMM program
    #[account(mut)]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// Stores init state for the lower tick
    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// Stores init state for the upper tick
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,
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

fn calculate_fee(
    pos: &PersonalPositionState,
    reward_infos: &Vec<raydium_amm_v3::states::RewardInfo>,
    token_mint_0: &Pubkey,
    token_mint_1: &Pubkey,
) -> (u64, u64) {
    let mut fee0 = pos.token_fees_owed_0;
    let mut fee1 = pos.token_fees_owed_1;
    for (i, reward) in pos.reward_infos.iter().enumerate() {
        let reward_mint = reward_infos[i].token_mint;
        if reward_mint == *token_mint_0 {
            fee0 += reward.reward_amount_owed;
        }
        if reward_mint == *token_mint_1 {
            fee1 += reward.reward_amount_owed;
        }
    }
    (fee0, fee1)
}

pub fn decrease_liquidity<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
    liquidity: u128,
    mint_amount_0: u64,
    mint_amount_1: u64,
    swap_to_token_mint: Pubkey,
    slippage_bps: u16,
    fee_percent: u16,
    convert_to_usdc: bool,
) -> Result<()> {
    // 校验手续费比例，最大 100%（10000 bps）
    require!(fee_percent <= 10_000, LpDepositError::InvalidFeePercent);
    require!(slippage_bps <= 10_000, LpDepositError::InvalidSlippage);
    require!(
        swap_to_token_mint == ctx.accounts.vault_0_mint.key()
            || swap_to_token_mint == ctx.accounts.vault_1_mint.key(),
        LpDepositError::InvalidDepositMint
    );

    let pool_state = ctx.accounts.pool_state.load_mut()?;
    let reward_infos = pool_state.reward_infos.to_vec();
    drop(pool_state);
    // -----------------------------------
    // BEFORE: 读取用户 Token ATA 余额（用于余额差计算）
    // -----------------------------------
    let user_token0_balance_before = ctx.accounts.user_token0_account.amount;
    let user_token1_balance_before = ctx.accounts.user_token1_account.amount;

    let (fee0_before, fee1_before) = calculate_fee(
        &ctx.accounts.personal_position,
        &reward_infos,
        &ctx.accounts.vault_0_mint.key(),
        &ctx.accounts.vault_1_mint.key(),
    );

    let cpi_program = ctx.accounts.raydium_clmm_program.to_account_info();
    let accounts = &ctx.accounts;

    let cpi_accounts = clmm_accounts::DecreaseLiquidityV2 {
        nft_owner: accounts.user.to_account_info(),
        nft_account: accounts.position_nft_account.to_account_info(),
        personal_position: accounts.personal_position.to_account_info(),
        pool_state: accounts.pool_state.to_account_info(),
        protocol_position: accounts.protocol_position.to_account_info(),
        token_vault_0: accounts.token_vault_0.to_account_info(),
        token_vault_1: accounts.token_vault_1.to_account_info(),
        tick_array_lower: accounts.tick_array_lower.to_account_info(),
        tick_array_upper: accounts.tick_array_upper.to_account_info(),
        recipient_token_account_0: accounts.user_token0_account.to_account_info(),
        recipient_token_account_1: accounts.user_token1_account.to_account_info(),
        token_program: accounts.token_program.to_account_info(),
        token_program_2022: accounts.token_program_2022.to_account_info(),
        memo_program: accounts.memo_program.to_account_info(),
        vault_0_mint: accounts.vault_0_mint.to_account_info(),
        vault_1_mint: accounts.vault_1_mint.to_account_info(),
    };
    // let remaining_account = ctx.remaining_accounts.to_vec();
    let cpi_ctx = CpiContext::new(cpi_program.clone(), cpi_accounts);
    // .with_remaining_accounts(remaining_account);

    clmm_cpi::decrease_liquidity_v2(cpi_ctx, liquidity, mint_amount_0, mint_amount_1)?;

    ctx.accounts.user_token0_account.reload()?;
    ctx.accounts.user_token1_account.reload()?;
    ctx.accounts.personal_position.reload()?;

    let user_token0_balance_after = ctx.accounts.user_token0_account.amount;
    let user_token1_balance_after = ctx.accounts.user_token1_account.amount;
    let (fee0_after, fee1_after) = calculate_fee(
        &ctx.accounts.personal_position,
        &reward_infos,
        &ctx.accounts.vault_0_mint.key(),
        &ctx.accounts.vault_1_mint.key(),
    );

    let token_fees_owed_0 = fee0_before
        .checked_sub(fee0_after)
        .ok_or(LpDepositError::MathOverflow)?;
    let token_fees_owed_1 = fee1_before
        .checked_sub(fee1_after)
        .ok_or(LpDepositError::MathOverflow)?;

    let user_token0_amount = user_token0_balance_after
        .checked_sub(user_token0_balance_before)
        .ok_or(LpDepositError::MathOverflow)?;
    let user_token1_amount = user_token1_balance_after
        .checked_sub(user_token1_balance_before)
        .ok_or(LpDepositError::MathOverflow)?;

    if convert_to_usdc {
        let is_token0 = ctx.accounts.vault_0_mint.key() == swap_to_token_mint;
        msg!(
            "is_token0: {}, vault_0_mint: {}, swap_to_token_mint: {}",
            is_token0,
            ctx.accounts.vault_0_mint.key(),
            swap_to_token_mint
        );

        let (swap_amount, fee_amount) = if is_token0 {
            (user_token1_amount, token_fees_owed_1)
        } else {
            (user_token0_amount, token_fees_owed_0)
        };
        let pool_state = ctx.accounts.pool_state.load_mut()?;
        let sqrt_price_x64 = pool_state.sqrt_price_x64;
        drop(pool_state);
        let fee_amount_out =
            utils::calc_min_amount_out(fee_amount, !is_token0, sqrt_price_x64, slippage_bps);

        let swap_other_amount_threshold =
            utils::calc_min_amount_out(swap_amount, !is_token0, sqrt_price_x64, slippage_bps);

        msg!(
            "swap_v2, swap_amount:{}, swap_other_amount_threshold: {}, is_token0: {}",
            swap_amount,
            swap_other_amount_threshold,
            is_token0
        );
        msg!(
            "balance,  user_token0_balance: {},  user_token1_balance: {}",
            user_token0_balance_after - user_token0_balance_before,
            user_token1_balance_after - user_token1_balance_before
        );
        if swap_amount > 0 {
            swap_v2(
                &ctx,
                swap_amount,
                swap_other_amount_threshold,
                0,
                !is_token0,
            )?;
        }

        ctx.accounts.user_token0_account.reload()?;
        ctx.accounts.user_token1_account.reload()?;

        msg!(
            "swap_v2_balance,  user_token0_balance: {},  user_token1_balance: {}",
            ctx.accounts.user_token0_account.amount - user_token0_balance_before,
            ctx.accounts.user_token1_account.amount - user_token1_balance_before
        );

        let (reward_amount, principal_amount) = if is_token0 {
            let fee = token_fees_owed_0
                .checked_add(fee_amount_out)
                .ok_or(LpDepositError::MathOverflow)?;
            let principal = ctx
                .accounts
                .user_token0_account
                .amount
                .checked_sub(user_token0_balance_before)
                .ok_or(LpDepositError::MathOverflow)?
                .checked_sub(fee)
                .ok_or(LpDepositError::MathOverflow)?;
            (fee, principal)
        } else {
            let fee = token_fees_owed_1
                .checked_add(fee_amount_out)
                .ok_or(LpDepositError::MathOverflow)?;
            let principal = ctx
                .accounts
                .user_token1_account
                .amount
                .checked_sub(user_token1_balance_before)
                .ok_or(LpDepositError::MathOverflow)?
                .checked_sub(fee)
                .ok_or(LpDepositError::MathOverflow)?;
            (fee, principal)
        };

        let integrator_fee = reward_amount
            .checked_mul((fee_percent) as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10000)
            .ok_or(LpDepositError::MathOverflow)?;

        transfer_fee(
            &ctx.accounts.user,
            if is_token0 {
                &ctx.accounts.user_token0_account
            } else {
                &ctx.accounts.user_token1_account
            },
            if is_token0 {
                &ctx.accounts.fee_token0_account
            } else {
                &ctx.accounts.fee_token1_account
            },
            if is_token0 {
                Some(&ctx.accounts.vault_0_mint)
            } else {
                Some(&ctx.accounts.vault_1_mint)
            },
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            integrator_fee,
        )?;
        emit!(DecreaseLiquidityEvent {
            user: ctx.accounts.user.key(),
            pool: ctx.accounts.pool_state.key(),
            token0_mint: ctx.accounts.vault_0_mint.key(),
            token1_mint: ctx.accounts.vault_1_mint.key(),
            principal_amount_0: if is_token0 { principal_amount } else { 0 },
            principal_amount_1: if is_token0 { 0 } else { principal_amount },
            integrator_fee_0: if is_token0 { integrator_fee } else { 0 },
            integrator_fee_1: if is_token0 { 0 } else { integrator_fee },
            reward_amount_0: if is_token0 {
                reward_amount - integrator_fee
            } else {
                0
            },
            reward_amount_1: if is_token0 {
                0
            } else {
                reward_amount - integrator_fee
            },
        });
    } else {
        let integrator_fee_0 = token_fees_owed_0
            .checked_mul((fee_percent) as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10000)
            .ok_or(LpDepositError::MathOverflow)?;

        let integrator_fee_1 = token_fees_owed_1
            .checked_mul((fee_percent) as u64)
            .ok_or(LpDepositError::MathOverflow)?
            .checked_div(10000)
            .ok_or(LpDepositError::MathOverflow)?;

        // 将用户应得费用的一部分转入集成方费用账户
        transfer_fee(
            &ctx.accounts.user,
            &ctx.accounts.user_token0_account,
            &ctx.accounts.fee_token0_account,
            Some(&ctx.accounts.vault_0_mint),
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            integrator_fee_0,
        )?;

        transfer_fee(
            &ctx.accounts.user,
            &ctx.accounts.user_token1_account,
            &ctx.accounts.fee_token1_account,
            Some(&ctx.accounts.vault_1_mint),
            &ctx.accounts.token_program,
            Some(&ctx.accounts.token_program_2022),
            integrator_fee_1,
        )?;
        let principal_amount_0 = user_token0_amount
            .checked_sub(token_fees_owed_0)
            .ok_or(LpDepositError::MathOverflow)?;
        let principal_amount_1 = user_token1_amount
            .checked_sub(token_fees_owed_1)
            .ok_or(LpDepositError::MathOverflow)?;
        let reward_amount_0 = token_fees_owed_0
            .checked_sub(integrator_fee_0)
            .ok_or(LpDepositError::MathOverflow)?;
        let reward_amount_1 = token_fees_owed_1
            .checked_sub(integrator_fee_1)
            .ok_or(LpDepositError::MathOverflow)?;

        emit!(DecreaseLiquidityEvent {
            user: ctx.accounts.user.key(),
            pool: ctx.accounts.pool_state.key(),
            token0_mint: ctx.accounts.vault_0_mint.key(),
            token1_mint: ctx.accounts.vault_1_mint.key(),
            principal_amount_0,
            principal_amount_1,
            integrator_fee_0,
            integrator_fee_1,
            reward_amount_0,
            reward_amount_1,
        });
    }

    Ok(())
}

fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: &Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
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

pub fn transfer_fee<'info>(
    signer: &Signer<'info>,
    from: &InterfaceAccount<'info, TokenAccount>,
    to: &InterfaceAccount<'info, TokenAccount>,
    mint: Option<&InterfaceAccount<'info, Mint>>,
    token_program: &Program<'info, Token>,
    token_program_2022: Option<&Program<'info, Token2022>>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let mut token_program_info = token_program.to_account_info();

    match (mint, token_program_2022) {
        (Some(mint), Some(token_program_2022)) => {
            if from.to_account_info().owner == token_program_2022.key {
                token_program_info = token_program_2022.to_account_info()
            }
            token_2022::transfer_checked(
                CpiContext::new(
                    token_program_info,
                    token_2022::TransferChecked {
                        from: from.to_account_info(),
                        to: to.to_account_info(),
                        authority: signer.to_account_info(),
                        mint: mint.to_account_info(),
                    },
                ),
                amount,
                mint.decimals,
            )?;
        }
        _ => token::transfer(
            CpiContext::new(
                token_program_info,
                token::Transfer {
                    from: from.to_account_info(),
                    to: to.to_account_info(),
                    authority: signer.to_account_info(),
                },
            ),
            amount,
        )?,
    }

    Ok(())
}
