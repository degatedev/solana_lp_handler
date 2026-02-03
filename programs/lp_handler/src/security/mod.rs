use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use raydium_amm_v3::states::PoolState;

use crate::require_log;
use crate::LpDepositError;

#[derive(Clone)]
pub struct SecurityPolicy {
    /// 默认禁止 delegate / close_authority（更贴近“不可滞留权限”的安全目标）
    pub forbid_delegate: bool,
    pub forbid_close_authority: bool,
}

impl SecurityPolicy {
    pub fn default_for_program() -> Self {
        Self {
            forbid_delegate: true,
            forbid_close_authority: true,
        }
    }
}

/// 读取 security_config PDA：
/// - 要求该 PDA 必须已初始化（program owner + 可解析数据）
fn load_security_config<'info>(
    accounts: &[AccountInfo<'info>],
) -> Result<Option<AccountInfo<'info>>> {
    let (expected, _) = Pubkey::find_program_address(&[crate::SECURITY_CONFIG_SEED], &crate::ID);
    let ai = accounts
        .iter()
        .find(|a| a.key() == expected)
        .ok_or(LpDepositError::SecurityPoolWhitelistMissing)?;

    // 未初始化（或已 close）：直接拒绝（要求必须配置）
    if is_uninitialized_account(ai) {
        return err!(LpDepositError::SecurityPoolWhitelistMissing);
    }

    require_keys_eq!(
        *ai.owner,
        crate::ID,
        LpDepositError::SecurityPoolWhitelistInvalid
    );
    Ok(Some(ai.clone()))
}

#[derive(Clone, Debug)]
pub struct TokenAccountSnapshot {
    pub token_program: Pubkey,
    pub mint: Pubkey,
    /// token authority（SPL/2022 的 `owner` 字段）
    pub owner: Pubkey,
    pub amount: u64,
    pub delegate: Option<Pubkey>,
    pub close_authority: Option<Pubkey>,
}

#[derive(Clone, Debug)]
pub struct SignerTokenAccountBefore<'info> {
    /// 该 token account 的 AccountInfo（可能来自 ctx.accounts 或 remaining_accounts）。
    ///
    /// 说明：exit 阶段需要读取“当前状态”做对账；仅保存 key 不足以在不借用 ctx 的情况下取回 AccountInfo。
    pub account: AccountInfo<'info>,
    pub delegate: Option<Pubkey>,
    pub close_authority: Option<Pubkey>,
    pub is_wsol_ata: bool,
}

#[derive(Clone, Debug)]
pub struct SecuritySnapshot<'info> {
    /// 入口时属于 signer 的 token accounts（仅对这部分做"权限未变更"对账）
    ///
    /// 注意：这里包含两部分：
    /// - ctx.accounts 中 authority==signer 的 token accounts
    /// - remaining_accounts 中 authority==signer 的 token accounts
    pub signer_token_accounts: Vec<SignerTokenAccountBefore<'info>>,
    /// 入口时未初始化(system owner + data_len=0)的账户索引，用于出口判断"新初始化账户"
    pub uninitialized_indices: Vec<usize>,
}

fn is_uninitialized_account(ai: &AccountInfo) -> bool {
    // 常见模式：未初始化账户 = system owner + data_len=0
    ai.owner == &anchor_lang::system_program::ID && ai.data_len() == 0
}

fn is_user_wsol_ata_address(user: &Pubkey, token_program: &Pubkey, ata: &Pubkey) -> bool {
    // wSOL = SPL Token native mint
    let wsol_mint_key = anchor_spl::token::spl_token::native_mint::ID;
    let expected = crate::derive_ata_address(
        user,
        &wsol_mint_key,
        token_program,
        &anchor_spl::associated_token::ID,
    );
    &expected == ata
}

fn parse_spl_token_account(ai: &AccountInfo) -> Option<TokenAccountSnapshot> {
    if ai.owner != &anchor_spl::token::ID {
        return None;
    }
    let data = ai.data.borrow();
    let acc = anchor_spl::token::spl_token::state::Account::unpack(&data).ok()?;
    Some(TokenAccountSnapshot {
        token_program: anchor_spl::token::ID,
        mint: acc.mint,
        owner: acc.owner,
        amount: acc.amount,
        delegate: acc.delegate.into(),
        close_authority: acc.close_authority.into(),
    })
}

fn parse_token2022_account(ai: &AccountInfo) -> Option<TokenAccountSnapshot> {
    if ai.owner != &anchor_spl::token_2022::ID {
        return None;
    }
    let data = ai.data.borrow();
    let state = anchor_spl::token_2022::spl_token_2022::extension::StateWithExtensions::<
        anchor_spl::token_2022::spl_token_2022::state::Account,
    >::unpack(&data)
    .ok()?;
    let acc = state.base;
    Some(TokenAccountSnapshot {
        token_program: anchor_spl::token_2022::ID,
        mint: acc.mint,
        owner: acc.owner,
        amount: acc.amount,
        delegate: acc.delegate.into(),
        close_authority: acc.close_authority.into(),
    })
}

fn parse_token_account(ai: &AccountInfo) -> Option<TokenAccountSnapshot> {
    parse_spl_token_account(ai).or_else(|| parse_token2022_account(ai))
}

fn security_config_pools_len(data: &[u8]) -> Result<usize> {
    // Anchor discriminator(8) + authority(32) + Vec len(u32=4) + pools
    const HEADER: usize = 8 + 32 + 4;
    if data.len() < HEADER {
        return err!(LpDepositError::SecurityPoolWhitelistInvalid);
    }
    let len_bytes: [u8; 4] = data[8 + 32..8 + 32 + 4]
        .try_into()
        .map_err(|_| error!(LpDepositError::SecurityPoolWhitelistInvalid))?;
    let n = u32::from_le_bytes(len_bytes) as usize;
    require!(
        n <= crate::MAX_ALLOWED_POOLS,
        LpDepositError::SecurityPoolWhitelistInvalid
    );
    let need = HEADER
        .checked_add(
            n.checked_mul(32)
                .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?,
        )
        .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?;
    require!(
        data.len() >= need,
        LpDepositError::SecurityPoolWhitelistInvalid
    );
    Ok(n)
}

fn security_config_contains(data: &[u8], n: usize, key: &Pubkey) -> bool {
    let start = 8 + 32 + 4;
    let kb = key.as_ref();
    for i in 0..n {
        let off = start + i * 32;
        if data[off..off + 32] == *kb {
            return true;
        }
    }
    false
}

fn security_config_fee_owners_len(data: &[u8], pools_n: usize) -> Result<usize> {
    // discriminator(8) + authority(32) + pools_len(4) + pools + fee_owners_len(4) + fee_owners
    let pools_start: usize = 8 + 32 + 4;
    let pools_end = pools_start
        .checked_add(
            pools_n
                .checked_mul(32)
                .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?,
        )
        .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?;
    let header2 = pools_end
        .checked_add(4)
        .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?;
    require!(
        data.len() >= header2,
        LpDepositError::SecurityPoolWhitelistInvalid
    );

    let len_bytes: [u8; 4] = data[pools_end..pools_end + 4]
        .try_into()
        .map_err(|_| error!(LpDepositError::SecurityPoolWhitelistInvalid))?;
    let n = u32::from_le_bytes(len_bytes) as usize;
    require!(
        n <= crate::MAX_FEE_OWNERS,
        LpDepositError::SecurityPoolWhitelistInvalid
    );
    let need = header2
        .checked_add(
            n.checked_mul(32)
                .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?,
        )
        .ok_or(LpDepositError::SecurityPoolWhitelistInvalid)?;
    require!(
        data.len() >= need,
        LpDepositError::SecurityPoolWhitelistInvalid
    );
    Ok(n)
}

fn security_config_fee_owner_contains(
    data: &[u8],
    pools_n: usize,
    fee_n: usize,
    key: &Pubkey,
) -> bool {
    let pools_start = 8 + 32 + 4;
    let pools_end = pools_start + pools_n * 32;
    let fee_start = pools_end + 4;
    let kb = key.as_ref();
    for i in 0..fee_n {
        let off = fee_start + i * 32;
        if data[off..off + 32] == *kb {
            return true;
        }
    }
    false
}

pub fn collect_accounts_to_check<'info>(
    mut ctx_accounts: Vec<AccountInfo<'info>>,
    remaining_accounts: &[AccountInfo<'info>],
) -> Vec<AccountInfo<'info>> {
    // 预留容量避免 extend 时触发二次分配（降低堆内存峰值）。
    // 说明：当前安全层主流程通常只扫描 `ctx.accounts`，此函数主要用于“确实需要把 remaining 合并成一个 Vec”
    // 的场景；如果你担心 OOM，优先使用“分两段循环分别扫描 main + remaining”的方式，避免一次性 Vec 峰值。
    ctx_accounts.reserve(remaining_accounts.len());
    ctx_accounts.extend_from_slice(remaining_accounts);
    // 注意：为降低 SBF 堆内存峰值，这里不再做去重（dedup）。
    // 可能会重复检查同一账户，但能显著减少 Vec 分配与峰值内存，降低 OOM 风险。
    ctx_accounts
}

/// 解析并返回本次指令应使用的安全策略。
///
/// 说明：
/// - 当前版本为**程序内常量配置**：不会从账户中读取 PDA 配置。
/// - 参数 `accounts` 仅用于保持 API 形态稳定（未来若恢复 PDA/治理配置可继续使用）。
pub fn resolve_policy<'info>(accounts: &[AccountInfo<'info>]) -> Result<SecurityPolicy> {
    let _ = accounts; // 纯常量配置：不从账户里读取任何 PDA 配置
    Ok(SecurityPolicy::default_for_program())
}

/// 入口检查并对所有可触达账户做快照，用于出口对账。
///
/// - `accounts`: 当前实现中为 `ctx.accounts.to_account_infos()`（安全层主扫描/快照对象）
/// - `remaining_accounts`: 原始 `ctx.remaining_accounts`
///   - 当前只做分隔符专项校验
///   - 另外：会把其中 **authority==signer 的 token accounts** 纳入快照，用于出口对账
/// - `pool_state`: 本次业务涉及的 pool_state（用于 pool 白名单校验）
/// - `signer`: 本次指令签名者（用于权限对账）
/// - `recipient`: 本次指令允许的收款方（用于 token authority 白名单逻辑）
pub fn entry_check_and_snapshot<'info>(
    accounts: &[AccountInfo<'info>],
    remaining_accounts: &[AccountInfo<'info>],
    pool_state: &AccountLoader<'info, PoolState>,
    signer: Pubkey,
    recipient: Pubkey,
    _policy: &SecurityPolicy,
) -> Result<SecuritySnapshot<'info>> {
    let pool_state_key = pool_state.key();

    // remaining_accounts 分隔符（crate::ID）约束：若出现，则必须唯一、只读
    let sep_cnt = remaining_accounts
        .iter()
        .filter(|a| a.key() == crate::ID)
        .count();
    if sep_cnt > 0 {
        require!(sep_cnt == 1, LpDepositError::SecuritySeparatorInvalid);
    }

    // Pool 白名单（强制启用）：
    // - 必须提供 security_config PDA（账户需在列表中）
    // - 必须已初始化
    // - pools 不能为空，且 pool_state 必须在 pools 内
    let cfg_ai_opt = load_security_config(accounts)?;
    let mut pools_n: usize = 0;
    let mut fee_owners_n: usize = 0;
    let mut cfg_ai_for_fee: Option<AccountInfo<'info>> = None;

    if let Some(cfg_ai) = cfg_ai_opt {
        // 先 clone，避免后续 data.borrow() 导致无法 move
        cfg_ai_for_fee = Some(cfg_ai.clone());
        let data = cfg_ai.data.borrow();
        pools_n = security_config_pools_len(&data)?;
        fee_owners_n = security_config_fee_owners_len(&data, pools_n)?;
        require!(pools_n != 0, LpDepositError::SecurityPoolWhitelistInvalid);
        require!(
            security_config_contains(&data, pools_n, &pool_state_key),
            LpDepositError::SecurityPoolNotAllowed
        );
    }

    // 轻量快照：
    // - signer 的 token accounts（用于出口对账 authority/delegate/close_authority）
    // - 入口时未初始化账户索引（用于出口判断“新初始化账户”）
    let mut signer_token_accounts: Vec<SignerTokenAccountBefore<'info>> = Vec::new();
    let mut uninitialized_indices: Vec<usize> = Vec::new();
    let pool_state_data = pool_state.load()?;

    for (idx, ai) in accounts.iter().enumerate() {
        // 可执行 program 白名单（按 key）
        if ai.executable {
            let ok = crate::ALLOWED_EXECUTABLE_PROGRAMS
                .iter()
                .any(|k| k == &ai.key());
            require_log!(
                ok,
                LpDepositError::SecurityUnauthorizedExecutableProgram,
                "ai.key={}",
                ai.key(),
            );
        }

        if is_uninitialized_account(ai) {
            uninitialized_indices.push(idx);
        }
        if let Some(ta) = parse_token_account(ai) {
            if ta.owner == signer {
                let token_program = ta.token_program;
                let is_wsol_ata = ta.mint == anchor_spl::token::spl_token::native_mint::ID
                    && is_user_wsol_ata_address(&signer, &token_program, &ai.key());
                signer_token_accounts.push(SignerTokenAccountBefore {
                    account: ai.clone(),
                    delegate: ta.delegate,
                    close_authority: ta.close_authority,
                    is_wsol_ata,
                });
                // 当前账户不是 signer 的账户，并且不是 recipient 的账户
            } else if ta.owner != recipient {
                // 非 signer/recipient 的 token account：限制其 authority 或 token account 地址必须在允许范围内。

                // token 的 owner 是否在 fee_owner 白名单（来自 security_config PDA）
                let is_fee_owner = if fee_owners_n != 0 {
                    // cfg_ai_for_fee 为 None 只可能发生在“未初始化”场景，此时 fee_owners_n 必为 0
                    let cfg_ai = cfg_ai_for_fee.as_ref().unwrap();
                    let data = cfg_ai.data.borrow();
                    security_config_fee_owner_contains(&data, pools_n, fee_owners_n, &ta.owner)
                } else {
                    false
                };

                let ata = ai.key();

                // 池子是否支持该 token account
                let mut is_pool_support_ta =
                    pool_state_data.token_vault_0 == ata || pool_state_data.token_vault_1 == ata;
                if !is_pool_support_ta {
                    for reward_info in pool_state_data.reward_infos.iter() {
                        if reward_info.initialized() && reward_info.token_vault == ata {
                            is_pool_support_ta = true;
                            break;
                        }
                    }
                }

                // authority 不是 fee_owner 且 token account 地址不是池子支持的 vault，则拒绝
                if !is_fee_owner && !is_pool_support_ta {
                    require_log!(
                        false,
                        LpDepositError::SecurityNonWhitelistTokenAccount,
                        "ta.mint={}, ta.owner={}",
                        ta.mint,
                        ta.owner,
                    );
                }
            }
        }
    }

    // 仅补充扫描 remaining_accounts 中“authority==signer”的 token accounts，用于出口对账覆盖 signer 的 token 账户权限变更。
    // （不对 remaining_accounts 做全量白名单扫描，以降低内存峰值）
    for ai in remaining_accounts.iter() {
        if let Some(ta) = parse_token_account(ai) {
            if ta.owner == signer {
                let token_program = ta.token_program;
                let is_wsol_ata = ta.mint == anchor_spl::token::spl_token::native_mint::ID
                    && is_user_wsol_ata_address(&signer, &token_program, &ai.key());
                signer_token_accounts.push(SignerTokenAccountBefore {
                    account: ai.clone(),
                    delegate: ta.delegate,
                    close_authority: ta.close_authority,
                    is_wsol_ata,
                });
            }
        }
    }
    Ok(SecuritySnapshot {
        signer_token_accounts,
        uninitialized_indices,
    })
}

/// 出口对账：对比入口快照与当前状态，确保不会滞留资产/权限未被恶意变更。
///
/// 重点检查：
/// - `owner == lp_handler` 的账户 lamports 不得异常沉淀（仅允许 rent-exempt）
/// - 用户 token account 的 authority/delegate/close_authority 不得被篡改（按策略）
/// - 新初始化账户的 owner(program id) 必须在允许集合
/// - 新初始化 token account 的 authority 必须在允许集合（当前实现：仅允许 `signer` 或 `recipient`）
pub fn exit_check<'info>(
    accounts: &[AccountInfo<'info>],
    signer: Pubkey,
    recipient: Pubkey,
    policy: &SecurityPolicy,
    before: SecuritySnapshot<'info>,
) -> Result<()> {
    let rent = Rent::get()?;
    // 允许的 token authority（用于“新初始化 token account”场景）：
    // - 若 authority == recipient：允许（业务指定收款方）
    // - 若 authority == signer：允许（默认收款方为签名者）
    #[inline(always)]
    fn is_allowed_authority(user: &Pubkey, candidate: &Pubkey, is_recipient: bool) -> bool {
        if is_recipient {
            return true;
        }
        if candidate == user {
            return true;
        }
        return false;
    }

    // 1) 约束：lp_handler 自己拥有的账户不应滞留多余 SOL（对本次账户集合内所有 owner==lp_handler 的账户）
    for ai in accounts.iter() {
        if ai.owner == &crate::ID {
            let min = rent.minimum_balance(ai.data_len());
            require!(
                ai.lamports() <= min,
                LpDepositError::SecurityProgramLamportsLeaked
            );
        }
    }

    // 2) signer token 账户权限对账（入口阶段 authority==signer 的 token accounts）
    for b in before.signer_token_accounts.iter() {
        let ai = &b.account;
        let cur_uninit = is_uninitialized_account(ai);
        let Some(cur) = parse_token_account(ai) else {
            // 例外：允许关闭 signer 的 wSOL ATA（unwrap wSOL 场景）
            if b.is_wsol_ata && cur_uninit {
                continue;
            }
            return err!(LpDepositError::SecurityTokenAccountCorrupted);
        };
        require_log!(
            cur.owner == signer,
            LpDepositError::SecurityTokenAuthorityChanged,
            "ai.key={}",
            ai.key(),
        );
        if policy.forbid_delegate {
            require_log!(
                cur.delegate.is_none(),
                LpDepositError::SecurityTokenDelegateNotAllowed,
                "ai.key={}",
                ai.key(),
            );
        } else {
            require_log!(
                cur.delegate == b.delegate,
                LpDepositError::SecurityTokenDelegateNotAllowed,
                "ai.key={}",
                ai.key(),
            );
        }
        if policy.forbid_close_authority {
            require_log!(
                cur.close_authority.is_none(),
                LpDepositError::SecurityTokenCloseAuthorityNotAllowed,
                "ai.key={}",
                ai.key(),
            );
        } else {
            require_log!(
                cur.close_authority == b.close_authority,
                LpDepositError::SecurityTokenCloseAuthorityNotAllowed,
                "ai.key={}",
                ai.key(),
            );
        }
    }

    // 3) 新初始化账户 owner 校验 + 新初始化 token account authority 校验
    for idx in before.uninitialized_indices.iter().copied() {
        let ai = accounts
            .get(idx)
            .ok_or(LpDepositError::SecurityAccountSetChanged)?;
        if is_uninitialized_account(ai) {
            continue;
        }
        // 新初始化账户 owner(program id) 必须合规
        require_log!(
            crate::ALLOWED_ACCOUNT_OWNERS.iter().any(|k| k == ai.owner),
            LpDepositError::SecurityDisallowedAccountOwner,
            "ai.key={}，ai.owner={}",
            ai.key(),
            ai.owner,
        );
        if let Some(ta) = parse_token_account(ai) {
            require_log!(
                is_allowed_authority(&signer, &ta.owner, &ta.owner == &recipient),
                LpDepositError::SecurityNewTokenAccountAuthorityInvalid,
                "ta.mint={}, ta.owner={}",
                ta.mint,
                ta.owner,
            );
        }
    }

    Ok(())
}
