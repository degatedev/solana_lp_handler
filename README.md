# lp_handler Contract Technical Documentation

## Table of Contents

- [1. Contract Overview and Positioning](#1-contract-overview-and-positioning)
- [2. ProgramId and External Dependencies](#2-programid-and-external-dependencies)
- [3. Overall Data Flow (Two Instructions)](#3-overall-data-flow-two-instructions)
- [4. Instruction 1: swap_and_deposit](#4-instruction-1-swap_and_deposit)
  * [4.1 Entry Point and Parameters](#41-entry-point-and-parameters)
  * [4.2 Account Model and Key Constraints](#42-account-model-and-key-constraints)
  * [4.3 Execution Steps](#43-execution-steps)
  * [4.4 remainingAccounts Rules](#44-remainingaccounts-rules)
  * [4.5 Reference Test Cases (Calling Convention)](#45-reference-test-cases-calling-convention)
- [5. Instruction 2: decrease_liquidity](#5-instruction-2-decrease_liquidity)
  * [5.1 Entry Point and Parameters](#51-entry-point-and-parameters)
  * [5.2 Account Model and Key Constraints](#52-account-model-and-key-constraints)
  * [5.3 remainingAccounts Separator Protocol (Critical)](#53-remainingaccounts-separator-protocol-critical)
  * [5.4 principal vs reward and the Fee Model (Critical)](#54-principal-vs-reward-and-the-fee-model-critical)
  * [5.5 Conversion Path convert_to_usdc=true (Implementation Semantics: Convert to Target Token)](#55-conversion-path-convert_to_usdctrue-implementation-semantics-convert-to-target-token)
- [6. Events (Observability)](#6-events-observability)
- [7. Error Codes and Common Troubleshooting](#7-error-codes-and-common-troubleshooting)
- [8. Trust Boundaries and Security Considerations](#8-trust-boundaries-and-security-considerations)
- [9. Mapping to Test Cases](#9-mapping-to-test-cases)
- [10. Example Transactions](#10-example-transactions)

## Reference Code Locations

- Program entry point: `programs/lp_handler/src/lib.rs`
- Deposit logic: `programs/lp_handler/src/instructions/swap_and_deposit.rs`
- Exit / claim logic: `programs/lp_handler/src/instructions/decrease_liquidity.rs`
- Events / errors / utility functions: `programs/lp_handler/src/state/`
- Usage examples: `tests/lp_deposit.test.ts`, `tests/lp_withdraw.test.ts`, `tests/lp_claim.test.ts`

---

## 1. Contract Overview and Positioning

`lp_handler` is a **Raydium CLMM (AmmV3) "composed transaction / position operation" wrapper layer**. It turns the two most common user-side actions into stable on-chain entry points:

- **Deposit side**: single-sided funds -> (swap if necessary) -> open a position and add liquidity
- **Exit / claim side**: decrease liquidity or claim -> (optionally swap to normalize first) -> **take a fee on rewards/trading fees only**

Core goals:

- **Reduce integration complexity**: chain multiple Raydium CLMM CPIs into a single instruction;
- **Built-in critical constraints**: tick range, mint validity, fee-recipient whitelist, consistency of Position NFT ATA derivation, and basic validation of remaining accounts;
- **Observability**: emit events for swap, liquidity increase, and liquidity decrease/claim.

---

## 2. ProgramId and External Dependencies

### ProgramId

`declare_id!` in `lib.rs` is fixed to the mainnet ProgramId (the repository comments note that this branch uses the mainnet address consistently).

### External Dependencies

- **Raydium CLMM**: `raydium_amm_v3`
  * CPIs: `swap_v2`, `open_position_with_token22_nft`, `decrease_liquidity_v2`
- **Anchor SPL**: Token / Token2022 / ATA / Memo
- **Internal contract utilities**: `programs/lp_handler/src/state/utils.rs`
  * Slippage: `calc_min_amount_out`, `apply_slippage_bps_floor`
  * Optimal swap ratio estimation: `calculate_optimal_swap_amount`
  * Principal calculation: `calculate_principal_amounts_for_liquidity`
  * ATA derivation: `derive_ata_address`
- **Fee whitelist**: `programs/lp_handler/src/state/consts.rs`

---

## 3. Overall Data Flow (Two Instructions)

#### `swap_and_deposit`

1. The user calls `swap_and_deposit(amount_0_in, amount_1_in, return_mint, tick_lower, tick_upper, slippage_bps, swap_amount_in, swap_min_out, swap_input_is_token0)`
2. The contract reads `pool_state` and decides on-chain the direction and amount of the swap to execute (at most one swap):

- **Out of range**: "single-sided deposit" is allowed; the contract overrides the off-chain plan and swaps the entire unneeded side into the needed side
- **Range straddles the current price**: the caller-supplied `swap_amount_in` / `swap_min_out` / `swap_input_is_token0` are used as the swap plan

3. If a swap is required: CPI into Raydium `swap_v2` (tick arrays / bitmap are supplied via remaining accounts)
4. CPI into Raydium `open_position_with_token22_nft` to mint the Position NFT (Token2022) and open the position
5. Handle the "leftovers": optionally normalize the leftovers into `return_mint`; if `min_out == 0`, handle them per the rules (non-wSOL → transfer to fee; wSOL → leave with the user and close/unwrap into SOL at the end)
6. Event: `IncreaseLiquidityEvent` (includes `return_amount_0` / `return_amount_1`)

#### `decrease_liquidity`

1. The user calls `decrease_liquidity(liquidity, mint_amount_0, mint_amount_1, swap_to_token_mint, slippage_bps, fee_percent, convert_to_usdc)`
2. The contract validates the `fee_owner` whitelist and the fee ATAs
3. CPI into Raydium `decrease_liquidity_v2` (reward-related remaining accounts come after the separator)
4. Split out the reward via "balance delta - principal_expected"; the fee is charged on the reward only
5. If `convert_to_usdc=true`: possibly CPI into `swap_v2` again to convert reward/principal into the target token before deducting the fee
6. Event: `DecreaseLiquidityEvent`

---

## 4. Instruction 1: `swap_and_deposit`

### 4.1 Entry Point and Parameters

Entry point: `programs/lp_handler/src/lib.rs` -> `instructions::swap_and_deposit`

Parameter semantics:

- `amount_0_in: u64`: maximum token0 input allowed for this call (smallest unit)
- `amount_1_in: u64`: maximum token1 input allowed for this call (smallest unit)
- `return_mint: Option<Pubkey>`: optional; if provided it must equal the pool's token0 or token1 mint, and is used to "normalize the leftovers into one side as far as possible"
- `tick_lower_index` / `tick_upper_index: i32`: the position's tick range; requires `lower < upper`
- `slippage_bps: u16`: slippage (bps); this instruction requires `< 5000`
- `swap_amount_in: u64` / `swap_min_out: u64` / `swap_input_is_token0: bool`:
  * When the range straddles the current price, these serve as the "off-chain plan" input (at most one swap is executed)
  * When out of range, the contract overrides this plan and instead swaps the entire unneeded side into the needed side (`slippage_bps` is still used to compute `min_out`)

### 4.2 Account Model and Key Constraints

Accounts: `SwapAndDeposit<'info>`

Key constraints (non-exhaustive):

- `raydium_clmm_program` must be `raydium_amm_v3::ID`
- `pool_state.load()?.amm_config == amm_config.key()`
- `token_vault_0/1` must match `pool_state.load()?.token_vault_0/1`
- The mint of `user_token0_account` must be `token_vault_0.mint`; likewise for `user_token1_account`
- **Position NFT (Token2022) ATA address validation**:
  * `position_nft_account` must equal `ATA(position_nft_owner, position_nft_mint, TOKEN_2022_PROGRAM_ID)`
  * Note: the contract only validates that the address derivation is correct; the ATA is allowed not to exist yet (it will be created by the CPI)

### 4.3 Execution Steps

1. Validate parameters: tick range, validity of `return_mint`, slippage upper bound
2. Read pool state: `sqrt_price_x64`, `tick_spacing`; determine whether the position is "out of range"
3. Decide the swap to execute (at most one):

- Out of range: the contract overrides the plan and executes a full "single-sided to double-sided" swap
- Range straddles the current price: use the plan parameters supplied by the caller

4. If a swap is needed: compute `min_out` and CPI into Raydium `swap_v2`
5. Compute `amount_0_max` / `amount_1_max` as the upper bounds for opening the position (post-swap balance + input budget)
6. CPI into Raydium `open_position_with_token22_nft`
7. Handle leftovers and emit the event: `swap_back_remaining_and_emit_increase_event`

- `return_mint=None`: leftovers on both sides stay in the user's token accounts
- `return_mint=Some(token0/token1)`: convert the other side's leftovers into the target token as far as possible; if `min_out == 0`:
  * Non-wSOL: that side's leftovers are transferred to `fee_token*_account`
  * wSOL: not transferred to fee; left with the user and closed/unwrapped into SOL at the end

8. Emit `IncreaseLiquidityEvent` (with the new `return_amount_0` / `return_amount_1` fields)

#### `return_amount_0` / `return_amount_1` semantics (case by case)

Inside `swap_back_remaining_and_emit_increase_event`:

- `leftover_0 = amount_0_max - spent_0`
- `leftover_1 = amount_1_max - spent_1`
- `return_amount_0` / `return_amount_1` record, in the event, the "leftovers ultimately returned to the user" (i.e. what stays in the user's `signer_token0/1_account`; if the mint is wSOL it will later be closed/unwrapped into SOL).

Case 1: `return_mint == None` (no requirement to normalize leftovers into one side)

- `return_amount_0 = leftover_0`
- `return_amount_1 = leftover_1`

Case 2: `return_mint == token0` (convert token1 leftovers into token0)

- **2.1** `leftover_1 == 0`
  * `return_amount_0 = leftover_0`
  * `return_amount_1 = 0`
- **2.2** `leftover_1 > 0` and `min_out > 0` (swap token1→token0 is executed)
  * `return_amount_0 = leftover_0 + out_from_swap`
  * `return_amount_1 = 0`
- **2.3** `leftover_1 > 0` and `min_out == 0` (no swap)
  * **2.3.a** token1 is not wSOL: `leftover_1` is transferred to fee
    + `return_amount_0 = leftover_0`
    + `return_amount_1 = 0`
  * **2.3.b** token1 is wSOL: `leftover_1` is not transferred to fee and stays with the user (later closed → SOL)
    + `return_amount_0 = leftover_0`
    + `return_amount_1 = leftover_1`

Case 3: `return_mint == token1` (convert token0 leftovers into token1)

- **3.1** `leftover_0 == 0`
  * `return_amount_1 = leftover_1`
  * `return_amount_0 = 0`
- **3.2** `leftover_0 > 0` and `min_out > 0` (swap token0→token1 is executed)
  * `return_amount_1 = leftover_1 + out_from_swap`
  * `return_amount_0 = 0`
- **3.3** `leftover_0 > 0` and `min_out == 0` (no swap)
  * **3.3.a** token0 is not wSOL: `leftover_0` is transferred to fee
    + `return_amount_1 = leftover_1`
    + `return_amount_0 = 0`
  * **3.3.b** token0 is wSOL: `leftover_0` is not transferred to fee and stays with the user (later closed → SOL)
    + `return_amount_1 = leftover_1`
    + `return_amount_0 = leftover_0`

Additional notes (wSOL & rent):

- `close_account` transfers away all lamports of the token account, so **the rent is transferred along with it**.
- The `return_amount_*` fields in the event record the token `amount` only (excluding rent); the SOL actually received by the wallet will be slightly more than that figure (it includes the rent).

### 4.4 remainingAccounts Rules

`ctx.remaining_accounts` in `swap_and_deposit` is **used only for Raydium `swap_v2`**:

- Structure: `[bitmap_extension?] + [swap_tick_array_0..N]`
- Tick arrays needed by open_position must not be mixed into remaining (lower/upper are already passed as fixed accounts)

The contract performs lightweight validation on remaining:

- Count `<= 32`
- Each account's `owner` must equal `raydium_clmm_program`

### 4.5 Reference Test Cases (Calling Convention)

Reference: `tests/lp_deposit.test.ts`

- Use the Raydium SDK to compute the tick arrays required for the swap and push them into `remainingAccounts` in order
- `positionNftMint` is generated with `Keypair.generate()` and used as a signer
- `positionNftAccount` uses Token2022 ATA derivation (`getATAAddress(user, mint, TOKEN_2022_PROGRAM_ID)`)
- The transaction layer usually raises the compute limit/price

---

## 5. Instruction 2: `decrease_liquidity`

### 5.1 Entry Point and Parameters

Entry point: `programs/lp_handler/src/lib.rs` -> `instructions::decrease_liquidity`

Parameter semantics:

- `liquidity: u128`
  * `>0`: typical withdraw (principal + rewards)
  * `=0`: typical claim (rewards only; principal is treated as 0)
- `mint_amount_0` / `mint_amount_1: u64`: parameters passed through to Raydium `decrease_liquidity_v2` (usually 0 in the tests)
- `swap_to_token_mint: Pubkey`: the target mint (only effective when `convert_to_usdc=true`); must be the pool's token0 or token1
- `slippage_bps: u16`: `<= 5000`
- `fee_percent: u16`: fee rate (bps), `<= 10000`
- `convert_to_usdc: bool`: the implementation semantics are "convert to the target token" (the name is business-oriented)

### 5.2 Account Model and Key Constraints

Accounts: `DecreaseLiquidity<'info>`

Key constraints:

- `fee_owner` must be present in the `fee_owners` whitelist in the `security_config` PDA (validated at runtime)
- `fee_token0_account` / `fee_token1_account` must be the **ATA** of the corresponding mint for fee_owner
  * `mint` and `owner` are both validated
  * During ATA derivation, the token_program used is the `owner` of the vault mint account (compatible with SPL Token / Token2022)

### 5.3 remainingAccounts Separator Protocol (Critical)

`decrease_liquidity` has to serve remaining accounts for two kinds of CPI at once:

- tick arrays / bitmap for swap_v2
- reward-related remaining accounts for decrease_liquidity_v2

The contract uses the **`lp_handler programId` as the separator**:

- Find the first position in `ctx.remaining_accounts` where `pubkey == crate::ID`
- Before the separator: `swap_remaining`
- After the separator (skipping the separator itself): `decrease_remaining`

A missing separator raises `InvalidRemainingAccounts` directly.

The order in which the reference tests (`tests/lp_withdraw.test.ts` / `tests/lp_claim.test.ts`) build remainingAccounts:

1. push swap_remaining (bitmap_extension? + tick arrays)
2. push the separator: `{ pubkey: program.programId, ... }`
3. push decrease_remaining (may include bitmap_extension plus a triplet per reward: poolRewardVault, ownerRewardVault, rewardMint)

### 5.4 principal vs reward and the Fee Model (Critical)

The key design point of this instruction: **the fee is charged on the reward (trading fees / rewards) only, never on the principal**.

Implementation:

- First CPI into Raydium `decrease_liquidity_v2`
- Obtain `delta0` / `delta1` from the **balance increase** of the user's token0/token1 ATAs
- Compute the principal:
  * `liquidity == 0`: principal is treated as 0 (claim semantics)
  * `liquidity > 0`: estimate `principal_expected_0/1` from the current price and the range
- `reward_gross = delta - principal_expected` (using `saturating_sub` to avoid negative values)
- Fee: `integrator_fee = reward_gross * fee_percent / 10000`

### 5.5 Conversion Path convert_to_usdc=true (Implementation Semantics: Convert to Target Token)

When `convert_to_usdc=true`:

- `swap_to_token_mint` determines the target side (token0 or token1)
- The conversion flow is "merged swap + approximate split":
  1. Merge this call's increase on the opposite side (principal + reward) into a single `swap_v2` conversion into the target token
  2. Split the swap output into `reward_out_est` / `principal_out_est` by approximate proportion
  3. The fee is still accrued on the reward portion only: `fee = (reward_direct + reward_out_est) * fee_percent`
- In this mode, `DecreaseLiquidityEvent` reports principal/reward/fee **consolidated on the target token side**, with the other side set to 0

When `convert_to_usdc=false`:

- No conversion takes place; the fee is charged separately on the token0 and token1 rewards and transferred separately to the fee_token0 / fee_token1 ATAs

---

## 6. Events (Observability)

Event definitions are in `programs/lp_handler/src/state/events.rs`:

- `IncreaseLiquidityEvent`: added amount0/amount1, tick range, position NFT mint, etc.
  * `return_amount_0` / `return_amount_1`: the leftovers ultimately returned to the user on each side (smallest unit; wSOL is closed/unwrapped into SOL at the end of the function, so the amount actually received additionally includes the rent)
- `DecreaseLiquidityEvent`: principal, reward, integrator_fee (reported either "post-conversion" or "per-token")

---

## 7. Error Codes and Common Troubleshooting

Error codes are in `programs/lp_handler/src/state/errors.rs`:

- `InvalidRemainingAccounts`
  * swap_and_deposit: remaining exceeds 32, or an owner is incorrect
  * decrease_liquidity: the separator cannot be found, or swap_remaining validation fails
- `InvalidPositionNftAccount`: the Position NFT ATA address derivation does not match
- `InvalidDepositMint`: `return_mint` (if provided) / `swap_to_token_mint` is not the pool's token0/token1
  * swap_and_deposit: also raised when `return_mint` is invalid (not the pool's token0/token1)
- `InvalidFeeOwner` / `InvalidFeeTokenAccount`: fee whitelist or fee ATA validation failed
- `InvalidTickRange`: illegal tick range
- `InvalidSlippage` / `InvalidFeePercent`: parameters out of bounds
- `NoBalanceChange`: after the decrease, both sides show zero increase (nothing to claim / nothing to withdraw)
- `MathOverflow` / `InvalidSqrtPrice`: arithmetic or price input anomaly

Troubleshooting suggestions:

- First confirm how remainingAccounts is organized (especially the separator for `decrease_liquidity`)
- Next confirm that fee_owner and the fee ATAs are correct (mint / owner / the token program used for derivation)
- Then check the Raydium CPI logs (whether the tick arrays / bitmap match the current pool)

---

## 8. Trust Boundaries and Security Considerations

### CPI Boundary

The contract depends heavily on the account validation and execution semantics of `raydium_amm_v3`. This contract mainly guarantees the "upper-layer protocol and business constraints":

- Consistency of the pool / vault / amm_config associations
- Consistency of Position NFT ATA address derivation
- Basic constraints on remainingAccounts (count and owner) plus the separator protocol for decrease
- Strong constraints on the fee whitelist and fee recipient accounts (preventing "transferring the fee back to oneself to bypass the charge")

### remainingAccounts Risk Surface

- The tick arrays / bitmap for swap_v2 are supplied by the caller: the contract only performs lightweight owner/count validation and does not parse whether they belong to the current pool; bad data will normally fail inside the Raydium CPI.
- The separator protocol in decrease_liquidity is critical to logical correctness: mixing them up or omitting the separator will fail outright or produce unexpected behavior.

### Slippage and Price Readings

At present `calc_min_amount_out` uses `sqrt_price_x64` to compute an "approximate threshold based on the current price". If the price moves violently or many ticks are crossed, this can still result in:

- Insufficient protection (min_out too low)
- Or over-strict protection causing failure (if the estimate is off)

To make this more rigorous, the implemented approach is: **re-read `pool_state.sqrt_price_x64` before every `swap_v2` CPI and then compute `min_out`** (read once before the reward swap and once before the principal swap in `decrease_liquidity`).

### Correctness Assumptions of reward-only Fee Charging

- The principal estimate relies on consistency between the current tick and sqrt_price; in extreme cases `delta < principal_expected` may occur. The current implementation uses `saturating_sub` so that reward = 0, which avoids charging a fee on the principal but does affect reward accounting.

---

## 9. Mapping to Test Cases

- `tests/lp_deposit.test.ts`: deposit (swap + open_position) and an example of building swap remainingAccounts
- `tests/lp_withdraw.test.ts`: decrease liquidity (including conversion + close position) and an example of the three-part remainingAccounts layout
- `tests/lp_claim.test.ts`: claim (`liquidity=0`) and an example of the three-part remainingAccounts layout

---

## 10. Example Transactions

- withdraw: <https://solscan.io/tx/4FP1XmPP16xFEUfnwhebY1GEskSzDbyQxNBCycpmUt6zPS1UVmdzkqB3P8J5C6BpgiXfYh2xz8KDSEG5AEdDcFRf>
- deposit: <https://solscan.io/tx/Tr5YioQQ5erjrvGBM87zmx5pmf6md3D8hiGdGcDXbPZye6rvSe891rBFJS38tCg3LGstgAtrWcF9dadiKDzrBys>
- claim: <https://solscan.io/tx/4V91uuLWYLwho7M6a9qh6CBUDNUJaxCUyTG1mCLfV8MjU57QoeYSwVZEccQe4rgtgPbAgWwAMtfHKGoesmzn4Gc>
