import * as anchor from '@coral-xyz/anchor';
import { BN } from 'bn.js';
import {
  ComputeBudgetProgram,
  Keypair,
  PublicKey,
  SYSVAR_RENT_PUBKEY,
  TransactionMessage,
  VersionedTransaction
} from '@solana/web3.js';
import {
  ApiV3PoolInfoConcentratedItem,
  CLMM_PROGRAM_ID,
  getATAAddress,
  getPdaExBitmapAccount,
  getPdaPersonalPositionAddress,
  getPdaProtocolPositionAddress,
  getPdaTickArrayAddress,
  MEMO_PROGRAM_ID,
  PoolUtils,
  PositionInfoLayout,
  SYSTEM_PROGRAM_ID,
  TickUtils
} from '@raydium-io/raydium-sdk-v2';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from '@solana/spl-token';
import { Program } from '@coral-xyz/anchor';
import { solveZapTwoSidedCLMM, ZapSwapDirection } from './utils';
import { LiquidityMath, SqrtPriceMath } from '@raydium-io/raydium-sdk-v2';
import amm_v3 from './amm_v3.json';
import {
  connection,
  deposit_amount,
  deposit_token_mint,
  ensureFunds,
  fee_address,
  getPoolInfo,
  getPoolKeys,
  getPriceRange,
  slippage as defaultSlippage,
  getRaydium,
  getTickArray,
  getTickLowerAndUpper,
  getTokenAta,
  pool_address,
  program,
  securityConfig,
  slippage,
  submitTransaction,
  user,
  userWallet
} from './help';

// 测试夹具：开仓 / 获取一个可用的 position。
// 模拟链上不支持按 owner 枚举 token 账户（getOwnerPositionInfo 拿不到本地新建的仓），
// 因此 withdraw / claim 统一通过这里现开一个仓再操作。

export async function swapAndDeposit(overrides?: { quotedMode?: number; quotedSqrtPriceX64?: anchor.BN }) {
  const raydium = await getRaydium();
  const poolKeys = await getPoolKeys();
  await ensureFunds();
  const { startPrice, endPrice } = await getPriceRange();
  const { tickLower, tickUpper } = getTickLowerAndUpper(poolKeys, startPrice, endPrice);
  const poolProgramId = new PublicKey(poolKeys.programId);

  const protocolPosition = getPdaProtocolPositionAddress(poolProgramId, pool_address, tickLower, tickUpper).publicKey;
  const { tickArrayLower, tickArrayUpper } = getTickArray(tickLower, tickUpper, poolKeys, poolProgramId);
  const [userToken0Account, userToken1Account, feeToken0Account, feeToken1Account] = await Promise.all([
    getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
    getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user),
    getTokenAta(connection, new PublicKey(poolKeys.mintA.address), fee_address, user),
    getTokenAta(connection, new PublicKey(poolKeys.mintB.address), fee_address, user)
  ]);

  const keypair = Keypair.generate();
  const positionNftMint = keypair.publicKey;
  // const metadataAccount = getPdaMetadataKey(positionNftMint);
  const isMintA = deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address));
  const positionNftAccount = getATAAddress(user, positionNftMint, TOKEN_2022_PROGRAM_ID);
  const personalPosition = getPdaPersonalPositionAddress(poolProgramId, positionNftMint);
  const poolInfo = await getPoolInfo();
  const computePool = await PoolUtils.fetchComputeClmmInfo({
    connection: raydium.connection,
    poolInfo: poolInfo
  });

  const [tickArrayCaches, epochInfo] = await Promise.all([
    PoolUtils.fetchMultiplePoolTickArrays({
      connection: raydium.connection,
      poolKeys: [computePool],
      batchRequest: true
    }),
    raydium.connection.getEpochInfo()
  ]);
  const tickArrayCache = tickArrayCaches[computePool.id.toBase58()];

  const res = await solveZapTwoSidedCLMM({
    tickArrayCache: tickArrayCache,
    computePool,
    epochInfo,
    tickLower: tickLower,
    tickUpper: tickUpper,
    amountAInBN: isMintA ? new BN(deposit_amount) : new BN(0),
    amountBInBN: !isMintA ? new BN(deposit_amount) : new BN(0),
    slippage: (slippage / 10_000).toString()
  });
  const tickArrayBitmapExtension = getPdaExBitmapAccount(poolProgramId, pool_address).publicKey;
  const remainingAccounts = [];

  if (tickArrayBitmapExtension) {
    remainingAccounts.push({
      pubkey: tickArrayBitmapExtension,
      isSigner: false,
      isWritable: true
    });
  }

  if (res.swapDirection !== ZapSwapDirection.None) {
    const clmmPoolInfo = await PoolUtils.fetchComputeClmmInfo({
      connection: raydium.connection,
      poolInfo
    });
    const tickCache = await PoolUtils.fetchMultiplePoolTickArrays({
      connection: raydium.connection,
      poolKeys: [clmmPoolInfo]
    });

    const swapAmountOut = await PoolUtils.computeAmountOutFormat({
      poolInfo: clmmPoolInfo,
      tickArrayCache: tickCache[pool_address.toBase58()],
      amountIn: res.swapAmountIN,
      tokenOut: poolInfo[res.swapDirection === ZapSwapDirection.AtoB ? 'mintB' : 'mintA'],
      slippage: 0,
      epochInfo: await raydium.fetchEpochInfo()
    });
    swapAmountOut.remainingAccounts.forEach((item) => {
      remainingAccounts.push({
        pubkey: item,
        isSigner: false,
        isWritable: true
      });
    });
  }

  const amount0In = isMintA ? new BN(deposit_amount) : new BN(0);
  const amount1In = !isMintA ? new BN(deposit_amount) : new BN(0);

  // 分隔符
  remainingAccounts.push({
    pubkey: program.programId,
    isSigner: false,
    isWritable: false
  });

  // action remaining accounts
  const tickArrayLowerStartIndex = TickUtils.getTickArrayStartIndexByTick(tickLower, poolInfo.config.tickSpacing);
  const tickArrayUpperStartIndex = TickUtils.getTickArrayStartIndexByTick(tickUpper, poolInfo.config.tickSpacing);

  if (
    PoolUtils.isOverflowDefaultTickarrayBitmap(poolInfo.config.tickSpacing, [
      tickArrayLowerStartIndex,
      tickArrayUpperStartIndex
    ]) &&
    tickArrayBitmapExtension
  ) {
    remainingAccounts.push({
      pubkey: tickArrayBitmapExtension,
      isSigner: false,
      isWritable: true
    });
  }
  const swapInputIsToken0 = res.swapDirection === ZapSwapDirection.AtoB;
  const quotedSqrtPriceX64 = overrides?.quotedSqrtPriceX64 ?? new BN(computePool.sqrtPriceX64.toString());
  // returnMint 可选：传入则会在链上把剩余统一换回该 mint；不传则不做剩余兑换
  const returnMint = deposit_token_mint;

  const instruction = await program.methods
    .swapAndDeposit(
      amount0In,
      amount1In,
      returnMint,
      tickLower,
      tickUpper,
      overrides?.quotedMode ?? res.quotedMode,
      quotedSqrtPriceX64,
      slippage,
      res.swapAmountIN,
      res.swapMinOut,
      swapInputIsToken0
    )
    .accountsStrict({
      raydiumClmmProgram: CLMM_PROGRAM_ID,
      memoProgram: MEMO_PROGRAM_ID,
      signer: user,
      feeOwner: fee_address,
      feeToken0Account: feeToken0Account.tokenAccount,
      feeToken1Account: feeToken1Account.tokenAccount,
      ammConfig: new PublicKey(poolKeys.config.id),
      poolState: pool_address,
      observationState: new PublicKey(poolKeys.observationId),
      signerToken0Account: userToken0Account.tokenAccount,
      signerToken1Account: userToken1Account.tokenAccount,
      recipient: user,
      refundToken0Account: userToken0Account.tokenAccount,
      refundToken1Account: userToken1Account.tokenAccount,
      positionNftMint,
      positionNftAccount: positionNftAccount.publicKey,
      protocolPosition,
      tickArrayLower,
      tickArrayUpper,
      personalPosition: personalPosition.publicKey,
      rent: SYSVAR_RENT_PUBKEY,
      systemProgram: SYSTEM_PROGRAM_ID,
      tokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      tokenProgram2022: TOKEN_2022_PROGRAM_ID,
      tokenVault0: new PublicKey(poolKeys.vault.A),
      tokenVault1: new PublicKey(poolKeys.vault.B),
      vault0Mint: new PublicKey(poolKeys.mintA.address),
      vault1Mint: new PublicKey(poolKeys.mintB.address),
      securityConfig
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
        ComputeBudgetProgram.setComputeUnitLimit({
          units: 1_200_000
        }),
        ComputeBudgetProgram.setComputeUnitPrice({
          microLamports: 1000
        }),
        userToken0Account.instruction,
        userToken1Account.instruction,
        instruction
      ]
    }).compileToV0Message(addressLookupTableAccounts)
  );
  const transactionResult = await submitTransaction(transaction, [keypair]);

  return {
    transactionResult,
    quotedMode: res.quotedMode,
    positionNftMint
  };
}

/// 开一个新仓并返回其 position NFT mint。交易失败时抛出，避免后续用例拿到无效 position。
export async function openPositionForTest(): Promise<PublicKey> {
  const { transactionResult, positionNftMint } = await swapAndDeposit();
  if (transactionResult.value.err !== null) {
    throw new Error(
      'openPositionForTest failed: ' +
        JSON.stringify(transactionResult.value.err) +
        '\n' +
        (transactionResult.value.logs || []).join('\n')
    );
  }
  return positionNftMint;
}

/// 读取链上 PersonalPosition，拼成测试需要的 position 形状。
export async function loadPosition(positionNftMint: PublicKey) {
  const poolKeys = await getPoolKeys();
  const poolProgramId = new PublicKey(poolKeys.programId);
  const ppKey = getPdaPersonalPositionAddress(poolProgramId, positionNftMint).publicKey;
  const info = await connection.getAccountInfo(ppKey);
  if (!info) throw new Error('PersonalPosition not found: ' + positionNftMint.toBase58());
  const decoded = PositionInfoLayout.decode(info.data);
  return {
    nftMint: positionNftMint,
    tickLower: decoded.tickLower,
    tickUpper: decoded.tickUpper,
    liquidity: decoded.liquidity
  };
}

export async function openPositionWithoutSwap(overrides?: {
  liquidityOverride?: InstanceType<typeof BN>;
  slippageBps?: number;
}) {
  const raydium = await getRaydium();
  const poolKeys = await getPoolKeys();
  await ensureFunds();
  // 单边只投 USDC：区间必须完全落在现价下方，否则 min(L0, L1) 恒为 0
  const { startPrice, endPrice } = await getPriceRange('belowSpot');
  const { tickLower, tickUpper } = getTickLowerAndUpper(poolKeys, startPrice, endPrice);
  const poolProgramId = new PublicKey(poolKeys.programId);

  const protocolPosition = getPdaProtocolPositionAddress(poolProgramId, pool_address, tickLower, tickUpper).publicKey;
  const { tickArrayLower, tickArrayUpper } = getTickArray(tickLower, tickUpper, poolKeys, poolProgramId);

  const [userToken0Account, userToken1Account] = await Promise.all([
    getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
    getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user)
  ]);

  const keypair = Keypair.generate();
  const positionNftMint = keypair.publicKey;
  const positionNftAccount = getATAAddress(user, positionNftMint, TOKEN_2022_PROGRAM_ID);
  const personalPosition = getPdaPersonalPositionAddress(poolProgramId, positionNftMint);

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
    .openPositionWithoutSwap(amount0In, amount1In, tickLower, tickUpper, liquidity, slippageBps)
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
      positionNftMint,
      positionNftAccount: positionNftAccount.publicKey,
      protocolPosition,
      personalPosition: personalPosition.publicKey,
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

  const transactionResult = await submitTransaction(transaction, [keypair]);

  return { transactionResult, computedLiquidity, positionNftMint };
}

/// 用"不含 swap"的方式开一个单边（现价下方区间）仓，供加仓类用例复用。
export async function openSingleSidedPositionForTest(): Promise<PublicKey> {
  const { transactionResult, positionNftMint } = await openPositionWithoutSwap();
  if (transactionResult.value.err !== null) {
    throw new Error(
      'openSingleSidedPositionForTest failed: ' +
        JSON.stringify(transactionResult.value.err) +
        '\n' +
        (transactionResult.value.logs || []).join('\n')
    );
  }
  return positionNftMint;
}
