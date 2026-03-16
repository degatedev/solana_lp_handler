# DeGate LP Handler Audit Remediation Decision Table

Notes:

* This document lists the handling decision for each issue in the audit report.
* `Fixed` means the issue has been remediated. `059613e` is a pre-existing commit; the remaining remediation changes are included in commit `09841f3039774e285039c0c478dc49e5c77e63e3`.
* `Acknowledged` means the issue was reviewed, but we intentionally chose not to implement the exact recommendation from the report, and we accept the residual risk with explanation.

| Issue | Decision | Commit Hash | Notes |
| --- | --- | --- | --- |
| `H01` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | The lamports check for program-owned accounts was changed from an absolute rent-based limit to an entry snapshot / exit relative comparison, preventing global DoS caused by external lamport transfers into PDAs. |
| `M01` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | Deposit / zap-side liquidity calculations are now transfer-fee-aware for Token-2022 mints, and the decrease path also reconciles expected principal against actual post-fee balance changes. |
| `M02` | `Fixed` | `059613e` | `quoted_mode` and `quoted_sqrt_price_x64` validation were introduced to prevent out-of-range automatic swaps from overriding user-provided execution protections. |
| `M03` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | The main zap swap now uses `quoted_mode`, `quoted_sqrt_price_x64`, `swap_min_out`, and `sqrt_price_limit_x64` tick-boundary enforcement together, preventing the automatic swap from continuing past the position boundary. Commit `059613e` added quote anchoring; this round added boundary price enforcement. |
| `M04` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `init_security_config` still requires a restricted bootstrap admin, but that admin is no longer hardcoded in the repository. The deploy script injects the current deployer wallet via the `SECURITY_ADMIN` environment variable before `anchor build`, which preserves restricted initialization while fitting the existing deployment flow. |
| `M05` | `Fixed` | `059613e` | Quote constraints and price protections were added to the claim / convert path to align it with the deposit path. |
| `L01` | `Acknowledged` | `-` | We split `remaining_accounts` into four sections: `main_swap_remaining`, `action_remaining`, `cleanup_swap_remaining_input_token0`, and `cleanup_swap_remaining_input_token1`. Cleanup swap now receives two candidate slices off-chain, and the program selects the one matching the actual leftover input direction instead of reusing the main swap path. Cleanup was also changed to best-effort: once the main swap and open/increase action succeed, a cleanup failure caused by CU limits, path mismatch, or state drift no longer reverts the main flow, and leftovers remain in the user accounts. However, the client still does not fully reproduce the post-action liquidity impact on subsequent tick-array path selection, so this item is not closed to the strictest standard from the report and the residual risk is accepted. |
| `L02` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | Dust detection now compares against the estimated target-side output rather than comparing arbitrary raw token amounts directly against `MIN_USDC_SWAP_AMOUNT`. |
| `L03` | `Acknowledged` | `-` | This branch is a retained product design choice: when the estimated output into the target token is too small and may lead to a zero-output or economically meaningless swap, the reward input is transferred directly to the fee address. We intentionally do not change this branch to a partial `fee_percent` charge because that would alter the product semantics. The residual risk is known and accepted. |
| `L04` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `fee_token0_account` and `fee_token1_account` are now required to be the canonical ATA for the corresponding mint owned by `fee_owner`, preventing fee fragmentation. |
| `L05` | `Acknowledged` | `-` | We relaxed `reward-only claim` so claim-only flows no longer revert due to `NoBalanceChange`, but we intentionally do not charge protocol fees on extra farming / incentive rewards. The protocol design only charges fees on the two pool assets, so foregoing fee revenue on external incentive tokens is an intentional product decision. |
| `E01` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `USDC_MIN` was renamed to the clearer `USDC_MINT`. |
| `E02` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | A unified helper was added so SPL Token native mint and Token-2022 native mint handling now follow consistent logic. |
| `E03` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | The unused dead code `collect_accounts_to_check` was removed to avoid misleading readers into thinking the security layer already scans all `remaining_accounts`. |
| `E04` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | Tick-array-related account constraints were unified so `swap_and_deposit`, `increase_liquidity`, and `decrease_liquidity` now align with the underlying Raydium CPI semantics. In particular, `swap_and_deposit` now accepts not-yet-initialized lower/upper tick array PDAs and validates their PDA addresses in-program, preventing legitimate opens from being rejected by an overly strict wrapper layer. |
| `E05` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `fee_owners` is now required to be non-empty in both `init_security_config` and `update_security_config`. |
| `E06` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `increase_liquidity` now enforces that the supplied `tick_lower_index` and `tick_upper_index` match the existing `personal_position`. |
| `E07` | `Fixed` | `09841f3039774e285039c0c478dc49e5c77e63e3` | `convert_to_usdc` was renamed to the more accurate `convert_to_target_mint`, and the documentation was updated accordingly. |

## Example Code Snippets

The following snippets are representative examples of the remediation approach only. The actual source of truth is the full committed diff.

### `H01` Program-Owned Lamports Check Changed to Entry Snapshot / Exit Relative Comparison

File: `programs/lp_handler/src/security/mod.rs`

```rust
#[derive(Clone, Debug)]
pub struct SecuritySnapshot {
    pub entries: Vec<SnapshotEntry>,
    pub program_owned_lamports: Vec<(Pubkey, u64)>,
}

fn program_owned_lamports_ok(
    accounts: &[AccountInfo<'_>],
    before: &[(Pubkey, u64)],
) -> bool {
    for (key, lamports_before) in before.iter() {
        let Some(ai) = accounts.iter().find(|a| a.key() == *key) else {
            return false;
        };
        if ai.lamports() > *lamports_before {
            return false;
        }
    }
    true
}
```

### `M01` Transfer-Fee-Aware Token Handling

File: `programs/lp_handler/src/state/utils.rs`

```rust
pub fn get_transfer_fee_for_amount(
    mint: &InterfaceAccount<Mint>,
    pre_fee_amount: u64,
) -> Result<u64> {
    if mint.to_account_info().owner == &anchor_spl::token::ID {
        return Ok(0);
    }

    get_transfer_fee_from_mint_info(mint.to_account_info(), pre_fee_amount)
}
```

File: `programs/lp_handler/src/instructions/zap_common.rs`

```rust
let amount_0_after_transfer_fee = amount_0_max
    .checked_sub(utils::get_transfer_fee_for_amount(accounts.vault_0_mint(), amount_0_max)?)
    .ok_or(LpDepositError::MathOverflow)?;
let amount_1_after_transfer_fee = amount_1_max
    .checked_sub(utils::get_transfer_fee_for_amount(accounts.vault_1_mint(), amount_1_max)?)
    .ok_or(LpDepositError::MathOverflow)?;
```

### `M03` Main Zap Swap Uses `sqrt_price_limit_x64` Boundary Protection

File: `programs/lp_handler/src/instructions/zap_common.rs`

```rust
fn derive_main_swap_price_limit(
    tick_lower_index: i32,
    tick_upper_index: i32,
    swap_input_is_token0: bool,
) -> Result<u128> {
    if swap_input_is_token0 {
        get_sqrt_price_at_tick(tick_lower_index)
    } else {
        get_sqrt_price_at_tick(tick_upper_index)
    }
}
```

```rust
let main_swap_price_limit_x64 =
    derive_main_swap_price_limit(tick_lower_index, tick_upper_index, swap_input_is_token0)?;

swap_v2_common(
    accounts,
    swap_amount_in,
    swap_min_out,
    main_swap_price_limit_x64,
    swap_input_is_token0,
    swap_remaining_slice.to_vec(),
)?;
```

### `M04` `SECURITY_ADMIN` Injected at Build Time by the Deploy Script

File: `programs/lp_handler/src/instructions/security_config.rs`

```rust
fn validate_security_config_init_authority(authority: Pubkey) -> Result<()> {
    require!(
        authority == crate::consts::SECURITY_ADMIN,
        LpDepositError::SecurityConfigAdminUnauthorized
    );
    Ok(())
}
```

File: `programs/lp_handler/build.rs`

```rust
let security_admin = env::var("SECURITY_ADMIN")
    .expect("SECURITY_ADMIN must be set before building lp_handler");
```

File: `scripts/deploy.ts`

```ts
const buildEnv = buildAnchorBuildEnv(userWallet.publicKey.toBase58());
// The deploy script explicitly passes buildEnv into anchor build
// so SECURITY_ADMIN is injected into the compilation environment.
runAnchorBuild(buildEnv);
```

### `L02` Dust Check Based on Target-Side Estimated Output

File: `programs/lp_handler/src/instructions/decrease_liquidity.rs`

```rust
fn should_skip_claim_only_dust_swap(
    principal_other_in: u64,
    swap_other_amount_threshold: u64,
) -> bool {
    principal_other_in == 0 && swap_other_amount_threshold < crate::consts::MIN_USDC_SWAP_AMOUNT
}
```

### `L04` Canonical ATA Enforcement for Fee Token Accounts

File: `programs/lp_handler/src/instructions/decrease_liquidity.rs`

```rust
fn validate_fee_token_account_keys(
    fee_owner: Pubkey,
    fee_token0_account: Pubkey,
    fee_token1_account: Pubkey,
    vault0_mint: Pubkey,
    vault1_mint: Pubkey,
    vault0_token_program: Pubkey,
    vault1_token_program: Pubkey,
) -> Result<()> {
    let expected_fee_token0 =
        utils::derive_ata_address(&fee_owner, &vault0_mint, &vault0_token_program);
    let expected_fee_token1 =
        utils::derive_ata_address(&fee_owner, &vault1_mint, &vault1_token_program);
    require!(fee_token0_account == expected_fee_token0, LpDepositError::InvalidFeeTokenAccount);
    require!(fee_token1_account == expected_fee_token1, LpDepositError::InvalidFeeTokenAccount);
    Ok(())
}
```

### `E03` Removal of Unused Dead Code

File: `programs/lp_handler/src/security/mod.rs`

```rust
// collect_accounts_to_check(...) removed
```

### `E06` `increase_liquidity` Tick Parameters Must Match the Existing Position

File: `programs/lp_handler/src/instructions/increase_liquidity.rs`

```rust
fn validate_position_ticks_match(
    personal_position: &PersonalPositionState,
    tick_lower_index: i32,
    tick_upper_index: i32,
) -> Result<()> {
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

### Additional Note: `swap_and_deposit` Now Accepts Uninitialized Tick Array PDAs

File: `programs/lp_handler/src/instructions/swap_and_deposit.rs`

```rust
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

    require_keys_eq!(tick_array_lower, expected_lower, LpDepositError::InvalidRemainingAccounts);
    require_keys_eq!(tick_array_upper, expected_upper, LpDepositError::InvalidRemainingAccounts);
    Ok(())
}
```

Explanation: Raydium `open_position_with_token22_nft` accepts lower / upper tick arrays in the form of correct PDAs even if they are not yet initialized, and may initialize them during CPI as needed. Previously, our `swap_and_deposit` wrapper tightened those accounts to `AccountLoader<TickArrayState>`, which caused legitimate opens to be rejected before reaching Raydium. The wrapper now accepts uninitialized PDAs while retaining PDA address validation to preserve account correctness.

### `L01` Cleanup Swap Uses Dual Candidate Account Slices

File: `programs/lp_handler/src/instructions/zap_common.rs`

```rust
let (
    swap_remaining_slice,
    action_remaining_slice,
    cleanup_swap_remaining_input_token0_slice,
    cleanup_swap_remaining_input_token1_slice,
) = split_zap_remaining_accounts(remaining_accounts)?;

let cleanup_swap_remaining = select_cleanup_remaining_accounts(
    cleanup_swap_remaining_input_token0_slice,
    cleanup_swap_remaining_input_token1_slice,
    actual_cleanup_input_is_token0,
);
```

Explanation: The current implementation no longer reuses the main swap `swap_remaining` for cleanup swap. The client now generates two candidate cleanup paths, one for `token0` input and one for `token1` input, and the program selects the matching path based on the actual leftover direction at runtime. The client-side quote for these candidates uses a conservative input bound based on the corresponding side's available upper bound, rather than treating the quoted `amountIn` as the exact refund size. This reduces the chance of missing tick arrays due to underestimating leftovers off-chain. In addition, cleanup swap is now best-effort: once the main swap and open/increase action have completed, a cleanup failure no longer reverts the main flow, and leftovers remain in the user accounts. However, the client still does not fully reproduce the impact of post-action liquidity on cleanup path selection, so this item remains `Acknowledged` rather than `Fixed`.

Additional note: the security entrypoint now receives an explicit `expected_separator_count` per instruction. `swap_and_deposit` and `increase_liquidity` use `3`, while `decrease_liquidity` uses `1`, preventing accidental over-acceptance or rejection across protocols.
