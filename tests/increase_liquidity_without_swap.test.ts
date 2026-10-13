import { BN } from 'bn.js';
import {
  ComputeBudgetProgram,
  PublicKey,
  SYSVAR_RENT_PUBKEY,
  TransactionMessage,
  VersionedTransaction
} from '@solana/web3.js';
import {
  CLMM_PROGRAM_ID,
  ClmmKeys,
  getATAAddress,
  getPdaExBitmapAccount,
  getPdaPersonalPositionAddress,
  getPdaProtocolPositionAddress,
  LiquidityMath,
  PoolUtils,
  Raydium,
  SqrtPriceMath,
  SYSTEM_PROGRAM_ID,
  TickUtils
} from '@raydium-io/raydium-sdk-v2';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from '@solana/spl-token';
import { loadPosition, openSingleSidedPositionForTest } from './fixtures';
import {
  connection,
  deposit_amount,
  deposit_token_mint,
  fee_address,
  getPoolInfo,
  getPoolKeys,
  getRaydium,
  getTickArray,
  getTokenAta,
  pool_address,
  program,
  securityConfig,
  slippage,
  submitTransaction,
  user
} from './help';

describe('increase_liquidity_without_swap', () => {
  let poolKeys: ClmmKeys;
  let raydium: Raydium;

  let positionNftMint: PublicKey;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
    // 现开一个仓供加仓用例使用（模拟链上无法按 owner 枚举已有 position）
    // 单边只投 USDC，需要一个区间完全在现价下方的仓
    positionNftMint = await openSingleSidedPositionForTest();
  }, 500000);

  async function simulateIncreaseLiquidityWithoutSwap(overrides?: {
    liquidityOverride?: InstanceType<typeof BN>;
    slippageBps?: number;
  }) {
    const poolProgramId = new PublicKey(poolKeys.programId);
    const position = await loadPosition(positionNftMint);
    if (!position) throw new Error(`no position found for pool ${pool_address.toBase58()}`);
    const tickLower = position.tickLower;
    const tickUpper = position.tickUpper;

    const protocolPosition = getPdaProtocolPositionAddress(poolProgramId, pool_address, tickLower, tickUpper).publicKey;
    const { tickArrayLower, tickArrayUpper } = getTickArray(tickLower, tickUpper, poolKeys, poolProgramId);

    const positionNftAccount = getATAAddress(user, position.nftMint, TOKEN_2022_PROGRAM_ID);

    const [userToken0Account, userToken1Account] = await Promise.all([
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user)
    ]);

    const poolInfo = await getPoolInfo();
    const computePool = await PoolUtils.fetchComputeClmmInfo({
      connection: raydium.connection,
      poolInfo
    });

    const isMintA = deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address));
    const amount0In = isMintA ? new BN(deposit_amount) : new BN(0);
    const amount1In = !isMintA ? new BN(deposit_amount) : new BN(0);

    // 链下按相同公式计算期望 liquidity（对应链上 compute_and_validate_liquidity）。
    // 注意：链上会先扣除 transfer fee 再计算 liquidity；此处未扣除，
    // 仅适用于无 transfer fee 的代币（如 USDC / SOL）。有 fee 扩展的 Token-2022 代币需在此扣费。
    const sqrtPriceLowerX64 = SqrtPriceMath.getSqrtPriceX64FromTick(tickLower);
    const sqrtPriceUpperX64 = SqrtPriceMath.getSqrtPriceX64FromTick(tickUpper);
    const computedLiquidity = LiquidityMath.getLiquidityFromTokenAmounts(
      computePool.sqrtPriceX64,
      sqrtPriceLowerX64,
      sqrtPriceUpperX64,
      amount0In,
      amount1In
    );
    const liquidity = overrides?.liquidityOverride ?? computedLiquidity;
    const slippageBps = overrides?.slippageBps ?? slippage;

    const tickArrayBitmapExtension = getPdaExBitmapAccount(poolProgramId, pool_address).publicKey;
    const tickArrayLowerStartIndex = TickUtils.getTickArrayStartIndexByTick(tickLower, poolInfo.config.tickSpacing);
    const tickArrayUpperStartIndex = TickUtils.getTickArrayStartIndexByTick(tickUpper, poolInfo.config.tickSpacing);
    const bitmapOverflows = PoolUtils.isOverflowDefaultTickarrayBitmap(poolInfo.config.tickSpacing, [
      tickArrayLowerStartIndex,
      tickArrayUpperStartIndex
    ]);

    // remaining_accounts 格式：[separator] [optional: bitmap_extension] [action_tick_arrays...]
    const remainingAccounts: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[] = [];
    remainingAccounts.push({ pubkey: program.programId, isSigner: false, isWritable: false });
    if (bitmapOverflows) {
      remainingAccounts.push({ pubkey: tickArrayBitmapExtension, isSigner: false, isWritable: true });
    }
    remainingAccounts.push({ pubkey: tickArrayLower, isSigner: false, isWritable: true });
    if (tickArrayLower.toBase58() !== tickArrayUpper.toBase58()) {
      remainingAccounts.push({ pubkey: tickArrayUpper, isSigner: false, isWritable: true });
    }

    const instruction = await program.methods
      .increaseLiquidityWithoutSwap(amount0In, amount1In, liquidity, slippageBps)
      .accountsStrict({
        raydiumClmmProgram: CLMM_PROGRAM_ID,
        recipient: user,
        signer: user,
        signerToken0Account: userToken0Account.tokenAccount,
        signerToken1Account: userToken1Account.tokenAccount,
        refundToken0Account: userToken0Account.tokenAccount,
        refundToken1Account: userToken1Account.tokenAccount,
        poolState: pool_address,
        feeOwner: fee_address,
        tokenVault0: new PublicKey(poolKeys.vault.A),
        tokenVault1: new PublicKey(poolKeys.vault.B),
        rent: SYSVAR_RENT_PUBKEY,
        systemProgram: SYSTEM_PROGRAM_ID,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        tokenProgram2022: TOKEN_2022_PROGRAM_ID,
        securityConfig,
        vault0Mint: new PublicKey(poolKeys.mintA.address),
        vault1Mint: new PublicKey(poolKeys.mintB.address),
        positionNftAccount: positionNftAccount.publicKey,
        personalPosition: getPdaPersonalPositionAddress(poolProgramId, position.nftMint).publicKey,
        protocolPosition,
        tickArrayLower,
        tickArrayUpper
      })
      .remainingAccounts(remainingAccounts)
      .instruction();

    let addressLookupTableAccounts = [];
    if (poolKeys.lookupTableAccount) {
      const res = await connection.getAddressLookupTable(new PublicKey(poolKeys.lookupTableAccount));
      if (res.value) {
        addressLookupTableAccounts.push(res.value);
      }
    }

    const transaction = new VersionedTransaction(
      new TransactionMessage({
        payerKey: user,
        recentBlockhash: (await connection.getLatestBlockhash()).blockhash,
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({ units: 1_200_000 }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
          userToken0Account.instruction,
          userToken1Account.instruction,
          instruction
        ]
      }).compileToV0Message(addressLookupTableAccounts)
    );

    const transactionResult = await submitTransaction(transaction);

    return { transactionResult, computedLiquidity };
  }

  it('increase liquidity without swap succeeds', async () => {
    const { transactionResult } = await simulateIncreaseLiquidityWithoutSwap();
    expect(transactionResult.value.err).toBeNull();
  }, 500000);

  it('rejects wildly wrong quoted liquidity', async () => {
    const { computedLiquidity } = await simulateIncreaseLiquidityWithoutSwap();
    // 传入 2 倍 liquidity，slippage=0 → LiquiditySlippageExceeded
    const { transactionResult } = await simulateIncreaseLiquidityWithoutSwap({
      liquidityOverride: computedLiquidity.muln(2),
      slippageBps: 0
    });

    expect(transactionResult.value.err).not.toBeNull();
    expect((transactionResult.value.logs || []).join('\n')).toContain('LiquiditySlippageExceeded');
  }, 500000);
});
