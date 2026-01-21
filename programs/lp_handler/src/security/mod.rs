use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_spl::token_2022::spl_token_2022::extension::BaseStateWithExtensions;

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
/// - 若账户未初始化（system owner + data_len=0），返回 None（视为“未启用白名单”，跳过 pool 校验）
/// - 若已初始化，返回 Some(SecurityConfig)
fn load_security_config<'info>(
    accounts: &[AccountInfo<'info>],
) -> Result<Option<crate::SecurityConfig>> {
    let (expected, _) = Pubkey::find_program_address(&[crate::SECURITY_CONFIG_SEED], &crate::ID);
    let ai = accounts
        .iter()
        .find(|a| a.key() == expected)
        .ok_or(LpDepositError::SecurityPoolWhitelistMissing)?;

    // 未初始化：直接跳过白名单校验
    if is_uninitialized_account(ai) {
        return Ok(None);
    }

    require_keys_eq!(
        *ai.owner,
        crate::ID,
        LpDepositError::SecurityPoolWhitelistInvalid
    );
    let data = ai.data.borrow();
    let mut d: &[u8] = &data;
    let cfg = crate::SecurityConfig::try_deserialize(&mut d)
        .map_err(|_| error!(LpDepositError::SecurityPoolWhitelistInvalid))?;
    Ok(Some(cfg))
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
pub struct UserTokenAccountBefore {
    /// 在 `accounts` 数组中的位置，避免 exit 阶段再做 key->AccountInfo 的映射/分配
    pub index: usize,
    pub token_program: Pubkey,
    pub mint: Pubkey,
    pub delegate: Option<Pubkey>,
    pub close_authority: Option<Pubkey>,
    pub is_wsol_ata: bool,
}

#[derive(Clone, Debug)]
pub struct SecuritySnapshot {
    /// 入口时属于 user 的 token accounts（仅对这部分做“权限未变更”对账）
    pub user_token_accounts: Vec<UserTokenAccountBefore>,
    /// 入口时未初始化(system owner + data_len=0)的账户索引，用于出口判断“新初始化账户”
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
    _policy: &SecurityPolicy,
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
                crate::ALLOWED_EXECUTABLE_PROGRAMS
                    .iter()
                    .any(|k| k == &ai.key()),
                LpDepositError::SecurityUnauthorizedExecutableProgram
            );
        }
    }

    // Pool 白名单：
    // - 必须提供 security_config PDA（账户需在列表中）
    // - 若 PDA 尚未初始化，跳过校验
    // - 若已初始化但 pools 为空，视为“未启用白名单”，跳过校验
    if let Some(cfg) = load_security_config(accounts)? {
        if !cfg.pools.is_empty() {
            for p in pool_states.iter() {
                require!(cfg.contains(p), LpDepositError::SecurityPoolNotAllowed);
            }
        }
    }

    // Token-2022 扩展风险检查（基于本次账户集合能观察到的 mint）
    // 先收集本次涉及的 Token-2022 mints（仅从 token-2022 token account 推导）
    let mut involved_token2022_mints: Vec<Pubkey> = Vec::new();
    for ai in accounts.iter() {
        if let Some(ta) = parse_token_account(ai) {
            if ta.token_program == anchor_spl::token_2022::ID
                && !involved_token2022_mints.iter().any(|m| m == &ta.mint)
            {
                involved_token2022_mints.push(ta.mint);
            }
        }
    }

    // Token-2022 风险扩展检查（能读到 mint account 就检查；读不到则跳过，不做 mint 白名单限制）
    for mint_key in involved_token2022_mints.iter() {
        for ai in accounts.iter() {
            if ai.key() != *mint_key {
                continue;
            }
            let Some(m) = parse_token2022_mint(ai) else {
                continue;
            };
            for ext in m.token2022_extensions.iter().copied() {
                require!(
                    !is_forbidden_token2022_extension(ext),
                    LpDepositError::SecurityToken2022ForbiddenExtension
                );
            }
        }
    }

    // 黑名单：不允许 user 或新增允许 authority 命中黑名单（防止绕过）
    if crate::USER_BLACKLIST.iter().any(|k| k == &user) {
        return err!(LpDepositError::SecurityBlacklistedUser);
    }
    for k in additional_allowed_token_authorities.iter() {
        if crate::USER_BLACKLIST.iter().any(|b| b == k) {
            return err!(LpDepositError::SecurityBlacklistedUser);
        }
    }

    // 轻量快照：只记录必要的 user token accounts + 未初始化账户索引
    let mut user_token_accounts: Vec<UserTokenAccountBefore> = Vec::new();
    let mut uninitialized_indices: Vec<usize> = Vec::new();
    for (idx, ai) in accounts.iter().enumerate() {
        if is_uninitialized_account(ai) {
            uninitialized_indices.push(idx);
        }
        if let Some(ta) = parse_token_account(ai) {
            // 入口阶段黑名单预检：authority/delegate/close_authority 不得命中黑名单
            if crate::USER_BLACKLIST.iter().any(|b| b == &ta.owner) {
                return err!(LpDepositError::SecurityBlacklistedUser);
            }
            if let Some(d) = ta.delegate {
                if crate::USER_BLACKLIST.iter().any(|b| b == &d) {
                    return err!(LpDepositError::SecurityBlacklistedUser);
                }
            }
            if let Some(ca) = ta.close_authority {
                if crate::USER_BLACKLIST.iter().any(|b| b == &ca) {
                    return err!(LpDepositError::SecurityBlacklistedUser);
                }
            }

            if ta.owner == user {
                let token_program = ta.token_program;
                let is_wsol_ata = ta.mint == anchor_spl::token::spl_token::native_mint::ID
                    && is_user_wsol_ata_address(&user, &token_program, &ai.key());
                user_token_accounts.push(UserTokenAccountBefore {
                    index: idx,
                    token_program,
                    mint: ta.mint,
                    delegate: ta.delegate,
                    close_authority: ta.close_authority,
                    is_wsol_ata,
                });
            }
        }
    }

    Ok(SecuritySnapshot {
        user_token_accounts,
        uninitialized_indices,
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
    let rent = Rent::get()?;

    // 允许的 token authority：user + 额外允许（比如 position_nft_owner）
    let mut allowed_token_authorities: Vec<Pubkey> = Vec::new();
    allowed_token_authorities.push(user);
    for k in additional_allowed_token_authorities.iter() {
        if !allowed_token_authorities.iter().any(|x| x == k) {
            allowed_token_authorities.push(*k);
        }
    }
    // 特权模式：当签名者 user 本身是 fee_owner 白名单地址时，允许其指定任意收款 authority
    // 用途：业务分账/代收（例如指定 position_nft_owner 或指定 decrease_liquidity 收款 token account 的 authority）
    let privileged_fee_owner_signer = crate::is_fee_owner(&user);

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

    // 2) 用户 token 账户权限对账（仅对入口阶段就是 user 的 token accounts）
    for b in before.user_token_accounts.iter() {
        let ai = accounts
            .get(b.index)
            .ok_or(LpDepositError::SecurityAccountSetChanged)?;
        let cur_uninit = is_uninitialized_account(ai);
        let Some(cur) = parse_token_account(ai) else {
            // 例外：允许关闭 user 的 wSOL ATA（unwrap wSOL 场景）
            if b.is_wsol_ata && cur_uninit {
                continue;
            }
            return err!(LpDepositError::SecurityTokenAccountCorrupted);
        };
        require!(
            cur.owner == user,
            LpDepositError::SecurityTokenAuthorityChanged
        );
        if policy.forbid_delegate {
            require!(
                cur.delegate.is_none(),
                LpDepositError::SecurityTokenDelegateNotAllowed
            );
        } else {
            require!(
                cur.delegate == b.delegate,
                LpDepositError::SecurityTokenDelegateNotAllowed
            );
        }
        if policy.forbid_close_authority {
            require!(
                cur.close_authority.is_none(),
                LpDepositError::SecurityTokenCloseAuthorityNotAllowed
            );
        } else {
            require!(
                cur.close_authority == b.close_authority,
                LpDepositError::SecurityTokenCloseAuthorityNotAllowed
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
        require!(
            crate::ALLOWED_ACCOUNT_OWNERS.iter().any(|k| k == ai.owner),
            LpDepositError::SecurityDisallowedAccountOwner
        );
        if let Some(ta) = parse_token_account(ai) {
            if !privileged_fee_owner_signer {
                require!(
                    allowed_token_authorities.iter().any(|k| k == &ta.owner),
                    LpDepositError::SecurityNewTokenAccountAuthorityInvalid
                );
            }
            // 黑名单检查
            require!(
                !crate::USER_BLACKLIST.iter().any(|b| b == &ta.owner),
                LpDepositError::SecurityBlacklistedUser
            );
            if let Some(d) = ta.delegate {
                if crate::USER_BLACKLIST.iter().any(|b| b == &d) {
                    return err!(LpDepositError::SecurityBlacklistedUser);
                }
            }
            if let Some(ca) = ta.close_authority {
                if crate::USER_BLACKLIST.iter().any(|b| b == &ca) {
                    return err!(LpDepositError::SecurityBlacklistedUser);
                }
            }
        }
    }

    Ok(())
}
