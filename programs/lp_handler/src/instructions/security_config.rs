use anchor_lang::prelude::*;

use crate::{
    LpDepositError, SecurityConfig, MAX_ALLOWED_POOLS, MAX_FEE_OWNERS, SECURITY_CONFIG_SEED,
};

pub fn init_security_config(
    ctx: Context<InitSecurityConfig>,
    pools: Vec<Pubkey>,
    fee_owners: Vec<Pubkey>,
) -> Result<()> {
    require!(!pools.is_empty(), LpDepositError::SecurityConfigPoolsEmpty);
    require!(
        pools.len() <= MAX_ALLOWED_POOLS,
        LpDepositError::MathOverflow
    );
    require!(
        fee_owners.len() <= MAX_FEE_OWNERS,
        LpDepositError::MathOverflow
    );
    let cfg = &mut ctx.accounts.security_config;
    cfg.authority = ctx.accounts.authority.key();
    cfg.pools = pools;
    cfg.fee_owners = fee_owners;
    Ok(())
}

pub fn update_security_config(
    ctx: Context<UpdateSecurityConfig>,
    pools: Vec<Pubkey>,
    fee_owners: Vec<Pubkey>,
) -> Result<()> {
    require!(!pools.is_empty(), LpDepositError::SecurityConfigPoolsEmpty);
    require!(
        pools.len() <= MAX_ALLOWED_POOLS,
        LpDepositError::MathOverflow
    );
    require!(
        fee_owners.len() <= MAX_FEE_OWNERS,
        LpDepositError::MathOverflow
    );
    let cfg = &mut ctx.accounts.security_config;
    cfg.pools = pools;
    cfg.fee_owners = fee_owners;
    Ok(())
}

pub fn close_security_config(_ctx: Context<CloseSecurityConfig>) -> Result<()> {
    Ok(())
}
#[derive(Accounts)]
pub struct InitSecurityConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = SecurityConfig::SIZE,
        seeds = [SECURITY_CONFIG_SEED],
        bump
    )]
    pub security_config: Account<'info, SecurityConfig>,

    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct UpdateSecurityConfig<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [SECURITY_CONFIG_SEED],
        bump,
        constraint = security_config.authority == authority.key()
    )]
    pub security_config: Account<'info, SecurityConfig>,
}

#[derive(Accounts)]
pub struct CloseSecurityConfig<'info> {
    /// 必须是当前 security_config.authority
    pub authority: Signer<'info>,

    /// 关闭 PDA，把 lamports 退回到 receiver
    #[account(
        mut,
        seeds = [SECURITY_CONFIG_SEED],
        bump,
        constraint = security_config.authority == authority.key(),
        close = receiver
    )]
    pub security_config: Account<'info, SecurityConfig>,

    /// CHECK: 接收退回租金的账户（通常是 authority）
    #[account(mut)]
    pub receiver: UncheckedAccount<'info>,
}
