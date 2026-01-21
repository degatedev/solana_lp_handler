use anchor_lang::prelude::*;

use crate::{admin, LpDepositError, SecurityConfig, MAX_ALLOWED_POOLS, SECURITY_CONFIG_SEED};

fn require_admin<'info>(authority: &Signer<'info>) -> Result<()> {
    require_keys_eq!(
        authority.key(),
        admin::ID,
        LpDepositError::SecurityConfigAdminUnauthorized
    );
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

pub fn init_security_config(ctx: Context<InitSecurityConfig>, pools: Vec<Pubkey>) -> Result<()> {
    require_admin(&ctx.accounts.authority)?;
    require!(
        pools.len() <= MAX_ALLOWED_POOLS,
        LpDepositError::MathOverflow
    );
    let cfg = &mut ctx.accounts.security_config;
    cfg.authority = ctx.accounts.authority.key();
    cfg.pools = pools;
    Ok(())
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

pub fn update_security_config(
    ctx: Context<UpdateSecurityConfig>,
    pools: Vec<Pubkey>,
) -> Result<()> {
    require_admin(&ctx.accounts.authority)?;
    require!(
        pools.len() <= MAX_ALLOWED_POOLS,
        LpDepositError::MathOverflow
    );
    let cfg = &mut ctx.accounts.security_config;
    cfg.pools = pools;
    Ok(())
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

pub fn close_security_config(_ctx: Context<CloseSecurityConfig>) -> Result<()> {
    require_admin(&_ctx.accounts.authority)?;
    // Anchor 的 `close = receiver` 会自动完成 lamports 转移与账户清理
    Ok(())
}
