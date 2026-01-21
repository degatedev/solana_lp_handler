use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_spl::token_2022::spl_token_2022::extension::BaseStateWithExtensions;

use crate::LpDepositError;

#[derive(Clone)]
pub struct SecurityPolicy {
    pub allowed_executable_programs: std::collections::BTreeSet<Pubkey>,
    pub allowed_account_owners: std::collections::BTreeSet<Pubkey>,
    pub allowed_pools: Option<std::collections::BTreeSet<Pubkey>>,
    pub allowed_token2022_mints: Option<std::collections::BTreeSet<Pubkey>>,
    pub user_blacklist: std::collections::BTreeSet<Pubkey>,
    /// 默认禁止 delegate / close_authority（更贴近“不可滞留权限”的安全目标）
    pub forbid_delegate: bool,
    pub forbid_close_authority: bool,
}

impl SecurityPolicy {
    pub fn default_for_program() -> Self {
        let allowed_exec: std::collections::BTreeSet<Pubkey> =
            crate::ALLOWED_EXECUTABLE_PROGRAMS.iter().copied().collect();

        let allowed_owners: std::collections::BTreeSet<Pubkey> =
            crate::ALLOWED_ACCOUNT_OWNERS.iter().copied().collect();

        // 常量配置：pool 白名单（空 = 不限制）
        let allowed_pools = if crate::ALLOWED_POOLS.is_empty() {
            None
        } else {
            Some(crate::ALLOWED_POOLS.iter().copied().collect())
        };

        // 常量配置：Token-2022 mint 白名单（空 = 不限制）
        let allowed_token2022_mints = if crate::ALLOWED_TOKEN2022_MINTS.is_empty() {
            None
        } else {
            Some(crate::ALLOWED_TOKEN2022_MINTS.iter().copied().collect())
        };

        // 常量配置：用户黑名单
        let user_blacklist = crate::USER_BLACKLIST.iter().copied().collect();

        Self {
            allowed_executable_programs: allowed_exec,
            allowed_account_owners: allowed_owners,
            allowed_pools,
            allowed_token2022_mints,
            user_blacklist,
            forbid_delegate: true,
            forbid_close_authority: true,
        }
    }
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
pub struct MintSnapshot {
    pub token_program: Pubkey,
    pub decimals: u8,
    pub mint_authority: Option<Pubkey>,
    pub freeze_authority: Option<Pubkey>,
    pub supply: u64,
    pub token2022_extensions: Vec<Token2022ExtensionType>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Token2022ExtensionType {
    PermanentDelegate,
    TransferHook,
    ConfidentialTransfer,
    NonTransferable,
    TransferFee,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct AccountSnapshot {
    pub key: Pubkey,
    pub owner: Pubkey,
    pub executable: bool,
    pub is_writable: bool,
    pub lamports: u64,
    pub data_len: usize,
    pub was_uninitialized: bool,
    pub token_account: Option<TokenAccountSnapshot>,
    pub mint: Option<MintSnapshot>,
}

#[derive(Clone, Debug)]
pub struct SecuritySnapshot {
    pub accounts: Vec<AccountSnapshot>,
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

fn parse_spl_mint(ai: &AccountInfo) -> Option<MintSnapshot> {
    if ai.owner != &anchor_spl::token::ID {
        return None;
    }
    let data = ai.data.borrow();
    let mint = anchor_spl::token::spl_token::state::Mint::unpack(&data).ok()?;
    Some(MintSnapshot {
        token_program: anchor_spl::token::ID,
        decimals: mint.decimals,
        mint_authority: mint.mint_authority.into(),
        freeze_authority: mint.freeze_authority.into(),
        supply: mint.supply,
        token2022_extensions: vec![],
    })
}

fn map_token2022_extension(
    t: anchor_spl::token_2022::spl_token_2022::extension::ExtensionType,
) -> Token2022ExtensionType {
    use anchor_spl::token_2022::spl_token_2022::extension::ExtensionType as E;
    match t {
        E::PermanentDelegate => Token2022ExtensionType::PermanentDelegate,
        E::TransferHook => Token2022ExtensionType::TransferHook,
        E::ConfidentialTransferMint | E::ConfidentialTransferAccount => {
            Token2022ExtensionType::ConfidentialTransfer
        }
        E::NonTransferable => Token2022ExtensionType::NonTransferable,
        E::TransferFeeConfig | E::TransferFeeAmount => Token2022ExtensionType::TransferFee,
        _ => Token2022ExtensionType::Unknown,
    }
}

fn parse_token2022_mint(ai: &AccountInfo) -> Option<MintSnapshot> {
    if ai.owner != &anchor_spl::token_2022::ID {
        return None;
    }
    let data = ai.data.borrow();
    let state = anchor_spl::token_2022::spl_token_2022::extension::StateWithExtensions::<
        anchor_spl::token_2022::spl_token_2022::state::Mint,
    >::unpack(&data)
    .ok()?;

    let ext_types = state
        .get_extension_types()
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(map_token2022_extension)
        .collect::<Vec<_>>();

    let mint = state.base;
    Some(MintSnapshot {
        token_program: anchor_spl::token_2022::ID,
        decimals: mint.decimals,
        mint_authority: mint.mint_authority.into(),
        freeze_authority: mint.freeze_authority.into(),
        supply: mint.supply,
        token2022_extensions: ext_types,
    })
}

fn parse_mint(ai: &AccountInfo) -> Option<MintSnapshot> {
    parse_spl_mint(ai).or_else(|| parse_token2022_mint(ai))
}

fn is_forbidden_token2022_extension(t: Token2022ExtensionType) -> bool {
    matches!(
        t,
        Token2022ExtensionType::PermanentDelegate
            | Token2022ExtensionType::TransferHook
            | Token2022ExtensionType::ConfidentialTransfer
            | Token2022ExtensionType::NonTransferable
    )
}

fn dedup_accounts<'info>(accounts: Vec<AccountInfo<'info>>) -> Vec<AccountInfo<'info>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(accounts.len());
    for a in accounts {
        if seen.insert(a.key()) {
            out.push(a);
        }
    }
    out
}

pub fn collect_accounts_to_check<'info>(
    mut ctx_accounts: Vec<AccountInfo<'info>>,
    remaining_accounts: &[AccountInfo<'info>],
) -> Vec<AccountInfo<'info>> {
    ctx_accounts.extend_from_slice(remaining_accounts);
    dedup_accounts(ctx_accounts)
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
/// - `accounts`: `ctx.accounts` + `ctx.remaining_accounts` 合并去重后的账户集合
/// - `remaining_accounts`: 原始 `ctx.remaining_accounts`（用于分隔符账户等专项检查）
/// - `pool_states`: 本次业务涉及的 pool_state（用于 pool 白名单校验）
/// - `user`: 本次指令签名者（用于黑名单/权限对账）
/// - `additional_allowed_token_authorities`: 业务允许出现的“新增 token account authority”补充白名单
pub fn entry_check_and_snapshot<'info>(
    accounts: &[AccountInfo<'info>],
    remaining_accounts: &[AccountInfo<'info>],
    pool_states: &[Pubkey],
    user: Pubkey,
    additional_allowed_token_authorities: &[Pubkey],
    policy: &SecurityPolicy,
) -> Result<SecuritySnapshot> {
    // remaining_accounts 分隔符（crate::ID）约束：若出现，则必须唯一、只读
    let sep_cnt = remaining_accounts
        .iter()
        .filter(|a| a.key() == crate::ID)
        .count();
    if sep_cnt > 0 {
        require!(sep_cnt == 1, LpDepositError::SecuritySeparatorInvalid);
        let sep_ai = remaining_accounts
            .iter()
            .find(|a| a.key() == crate::ID)
            .ok_or(LpDepositError::SecuritySeparatorInvalid)?;
        require!(sep_ai.executable, LpDepositError::SecuritySeparatorInvalid);
        require!(
            !sep_ai.is_writable,
            LpDepositError::SecuritySeparatorInvalid
        );
        require!(!sep_ai.is_signer, LpDepositError::SecuritySeparatorInvalid);
    }

    // 可执行 program 白名单（按 key）
    for ai in accounts.iter() {
        if ai.executable {
            require!(
                policy.allowed_executable_programs.contains(&ai.key()),
                LpDepositError::SecurityUnauthorizedExecutableProgram
            );
        }
    }

    // Pool 白名单（由入口显式传入需要校验的 pool_state 列表）
    if let Some(allowed_pools) = &policy.allowed_pools {
        for p in pool_states.iter() {
            require!(
                allowed_pools.contains(p),
                LpDepositError::SecurityPoolNotAllowed
            );
        }
    }

    // Token-2022 扩展风险检查（基于本次账户集合能观察到的 mint）
    let mut mint_by_key: std::collections::BTreeMap<Pubkey, MintSnapshot> =
        std::collections::BTreeMap::new();
    for ai in accounts.iter() {
        if let Some(m) = parse_mint(ai) {
            mint_by_key.insert(ai.key(), m);
        }
    }

    // 先扫描 token accounts 拿到涉及的 mints
    let mut involved_mints: std::collections::BTreeSet<Pubkey> = std::collections::BTreeSet::new();
    for ai in accounts.iter() {
        if let Some(ta) = parse_token_account(ai) {
            involved_mints.insert(ta.mint);
        }
    }

    for mint_key in involved_mints.iter() {
        if let Some(allowed_22) = &policy.allowed_token2022_mints {
            // 若该 mint 是 Token-2022 mint，则要求在 allowed_22 中（前提：本次能读到 mint）
            if let Some(m) = mint_by_key.get(mint_key) {
                if m.token_program == anchor_spl::token_2022::ID {
                    require!(
                        allowed_22.contains(mint_key),
                        LpDepositError::SecurityTokenMintNotAllowed
                    );
                }
            }
        }
        // Token-2022 风险扩展：要求本次交易提供 mint account，且不能带高风险扩展
        if let Some(m) = mint_by_key.get(mint_key) {
            if m.token_program == anchor_spl::token_2022::ID {
                for ext in m.token2022_extensions.iter().copied() {
                    require!(
                        !is_forbidden_token2022_extension(ext),
                        LpDepositError::SecurityToken2022ForbiddenExtension
                    );
                }
            }
        } else {
            // 如果 mint 在本次涉及且 token account 可能是 2022，但 mint 没传入，则无法做扩展风控
            // 这里采用保守策略：强制要求提供 mint account（否则拒绝）
            // 为避免误杀 SPL Token（mint 未传入但仍可安全），仅当 mint 处于 Token-2022 allowlist 模式时才强制
            if policy.allowed_token2022_mints.is_some() {
                return err!(LpDepositError::SecurityMissingToken2022MintAccount);
            }
        }
    }

    // 黑名单：不允许 user 或新增允许 authority 命中黑名单（防止绕过）
    if policy.user_blacklist.contains(&user) {
        return err!(LpDepositError::SecurityBlacklistedUser);
    }
    for k in additional_allowed_token_authorities.iter() {
        if policy.user_blacklist.contains(k) {
            return err!(LpDepositError::SecurityBlacklistedUser);
        }
    }

    let mut snapshots = Vec::with_capacity(accounts.len());
    for ai in accounts.iter() {
        let token_account = parse_token_account(ai);
        let mint = parse_mint(ai);
        snapshots.push(AccountSnapshot {
            key: ai.key(),
            owner: *ai.owner,
            executable: ai.executable,
            is_writable: ai.is_writable,
            lamports: ai.lamports(),
            data_len: ai.data_len(),
            was_uninitialized: is_uninitialized_account(ai),
            token_account,
            mint,
        });
    }

    // 额外：入口阶段可以提前限制“token account owner/authority 不能是黑名单”
    for s in snapshots.iter() {
        if let Some(ta) = &s.token_account {
            if policy.user_blacklist.contains(&ta.owner) {
                return err!(LpDepositError::SecurityBlacklistedUser);
            }
            if let Some(d) = ta.delegate {
                if policy.user_blacklist.contains(&d) {
                    return err!(LpDepositError::SecurityBlacklistedUser);
                }
            }
            if let Some(ca) = ta.close_authority {
                if policy.user_blacklist.contains(&ca) {
                    return err!(LpDepositError::SecurityBlacklistedUser);
                }
            }
        }
    }

    Ok(SecuritySnapshot {
        accounts: snapshots,
    })
}

/// 出口对账：对比入口快照与当前状态，确保不会滞留资产/权限未被恶意变更。
///
/// 重点检查：
/// - `owner == lp_handler` 的账户 lamports 不得异常沉淀（仅允许 rent-exempt）
/// - 用户 token account 的 authority/delegate/close_authority 不得被篡改（按策略）
/// - 新初始化账户的 owner(program id) 必须在允许集合
/// - 新初始化 token account 的 authority 必须在允许集合（**当 user 是 fee_owner 白名单时放宽此项**）
pub fn exit_check<'info>(
    accounts: &[AccountInfo<'info>],
    user: Pubkey,
    additional_allowed_token_authorities: &[Pubkey],
    policy: &SecurityPolicy,
    before: SecuritySnapshot,
) -> Result<()> {
    let mut current_by_key: std::collections::BTreeMap<Pubkey, AccountSnapshot> =
        std::collections::BTreeMap::new();
    for ai in accounts.iter() {
        current_by_key.insert(
            ai.key(),
            AccountSnapshot {
                key: ai.key(),
                owner: *ai.owner,
                executable: ai.executable,
                is_writable: ai.is_writable,
                lamports: ai.lamports(),
                data_len: ai.data_len(),
                was_uninitialized: is_uninitialized_account(ai),
                token_account: parse_token_account(ai),
                mint: parse_mint(ai),
            },
        );
    }

    let rent = Rent::get()?;

    // 允许的 token authority：user + 额外允许（比如 position_nft_owner）
    let mut allowed_token_authorities = std::collections::BTreeSet::new();
    allowed_token_authorities.insert(user);
    for k in additional_allowed_token_authorities.iter() {
        allowed_token_authorities.insert(*k);
    }
    // 特权模式：当签名者 user 本身是 fee_owner 白名单地址时，允许其指定任意收款 authority
    // 用途：业务分账/代收（例如指定 position_nft_owner 或指定 decrease_liquidity 收款 token account 的 authority）
    let privileged_fee_owner_signer = crate::is_fee_owner(&user);

    for b in before.accounts.iter() {
        let Some(a) = current_by_key.get(&b.key) else {
            // 本次账户集合是闭包，正常情况下不会消失；若消失就保守拒绝
            return err!(LpDepositError::SecurityAccountSetChanged);
        };

        // 1) 约束：lp_handler 自己拥有的账户不应滞留多余 SOL
        if b.owner == crate::ID || a.owner == crate::ID {
            // 允许 rent-exempt 最小值；不允许额外沉淀
            let min = rent.minimum_balance(a.data_len);
            require!(
                a.lamports <= min,
                LpDepositError::SecurityProgramLamportsLeaked
            );
        }

        // 2) 用户 token 账户权限不应被改变（authority/delegate/close_authority）
        if let Some(bta) = &b.token_account {
            if bta.owner == user {
                let Some(ata) = &a.token_account else {
                    // 例外：允许关闭 user 的 wSOL ATA（unwrap wSOL 场景）
                    // - 入口：该账户是 user 的 wSOL ATA
                    // - 出口：该账户变为未初始化（system owner + data_len=0）
                    if bta.mint == anchor_spl::token::spl_token::native_mint::ID
                        && a.was_uninitialized
                        && is_user_wsol_ata_address(&user, &bta.token_program, &b.key)
                    {
                        continue;
                    }
                    return err!(LpDepositError::SecurityTokenAccountCorrupted);
                };
                require!(
                    ata.owner == user,
                    LpDepositError::SecurityTokenAuthorityChanged
                );
                // delegate
                if policy.forbid_delegate {
                    require!(
                        ata.delegate.is_none(),
                        LpDepositError::SecurityTokenDelegateNotAllowed
                    );
                } else {
                    require!(
                        ata.delegate == bta.delegate,
                        LpDepositError::SecurityTokenDelegateNotAllowed
                    );
                }
                // close_authority
                if policy.forbid_close_authority {
                    require!(
                        ata.close_authority.is_none(),
                        LpDepositError::SecurityTokenCloseAuthorityNotAllowed
                    );
                } else {
                    require!(
                        ata.close_authority == bta.close_authority,
                        LpDepositError::SecurityTokenCloseAuthorityNotAllowed
                    );
                }
            }
        }

        // 3) 新增账户归属检查：入口未初始化 → 出口已初始化
        if b.was_uninitialized && !a.was_uninitialized {
            // 数据账户 owner(program id) 必须在允许 owner 集合（或仍是 system owner 的情况已经排除）
            require!(
                policy.allowed_account_owners.contains(&a.owner),
                LpDepositError::SecurityDisallowedAccountOwner
            );
            // 如果变成 token account，则其 authority 必须在允许集合，并且不在黑名单
            if let Some(ta) = &a.token_account {
                if !privileged_fee_owner_signer {
                    require!(
                        allowed_token_authorities.contains(&ta.owner),
                        LpDepositError::SecurityNewTokenAccountAuthorityInvalid
                    );
                }
                require!(
                    !policy.user_blacklist.contains(&ta.owner),
                    LpDepositError::SecurityBlacklistedUser
                );
                if let Some(d) = ta.delegate {
                    if policy.user_blacklist.contains(&d) {
                        return err!(LpDepositError::SecurityBlacklistedUser);
                    }
                }
                if let Some(ca) = ta.close_authority {
                    if policy.user_blacklist.contains(&ca) {
                        return err!(LpDepositError::SecurityBlacklistedUser);
                    }
                }
            }
        }
    }

    Ok(())
}
