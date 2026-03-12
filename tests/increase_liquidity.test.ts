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
  ClmmKeys,
  getATAAddress,
  getPdaExBitmapAccount,
  getPdaPersonalPositionAddress,
  getPdaProtocolPositionAddress,
  getPdaTickArrayAddress,
  MEMO_PROGRAM_ID,
  PoolUtils,
  Raydium,
  SYSTEM_PROGRAM_ID,
  TickUtils
} from '@raydium-io/raydium-sdk-v2';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from '@solana/spl-token';
import { solveZapTwoSidedCLMM } from './utils';
import {
  connection,
  deposit_amount,
  deposit_token_mint,
  endPrice,
  fee_address,
  getPoolInfo,
  getPoolKeys,
  getRaydium,
  getTickArray,
  getTickLowerAndUpper,
  getTokenAta,
  pool_address,
  program,
  securityConfig,
  slippage,
  startPrice,
  user,
  userWallet
} from './help';
import { Program } from '@coral-xyz/anchor';
import amm_v3 from './amm_v3.json';
import { ZapSwapDirection } from './utils';

// 环境变量已在 tests/setup.ts 中配置
// 如果需要覆盖，可以在这里设置：
// process.env.ANCHOR_PROVIDER_URL = "your-custom-rpc-url";
// process.env.ANCHOR_WALLET = "your-wallet-path";

describe('lp_increase_liquidity', () => {
  // Configure the client to use the local cluster.

  let poolKeys: ClmmKeys;
  let raydium: Raydium;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
  }, 50000);

  it('lp_deposit test', async () => {
    // Add your test here.
    const { tickLower, tickUpper } = getTickLowerAndUpper(poolKeys, startPrice, endPrice);
    const poolProgramId = new PublicKey(poolKeys.programId);
    const allPosition = await raydium.clmm.getOwnerPositionInfo({ programId: CLMM_PROGRAM_ID });
    const position = allPosition.shift();
    const protocolPosition = getPdaProtocolPositionAddress(poolProgramId, pool_address, tickLower, tickUpper).publicKey;
    const { tickArrayLower, tickArrayUpper } = getTickArray(tickLower, tickUpper, poolKeys, poolProgramId);
    const [userToken0Account, userToken1Account, feeToken0Account, feeToken1Account] = await Promise.all([
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), fee_address, user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), fee_address, user)
    ]);

    // const metadataAccount = getPdaMetadataKey(positionNftMint);

    const positionNftAccount = getATAAddress(user, position.nftMint, TOKEN_2022_PROGRAM_ID);
    const personalPosition = getPdaPersonalPositionAddress(poolProgramId, position.nftMint);
    const poolInfo = await getPoolInfo();
    const isMintA = deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address));

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

    remainingAccounts.push({
      pubkey: program.programId,
      isSigner: false,
      isWritable: false
    });

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
    const amount0In = isMintA ? new BN(deposit_amount) : new BN(0);
    const amount1In = !isMintA ? new BN(deposit_amount) : new BN(0);
    const swapInputIsToken0 = res.swapDirection === ZapSwapDirection.AtoB;
    // returnMint 可选：传入则会在链上把剩余统一换回该 mint；不传则不做剩余兑换
    const returnMint = deposit_token_mint;

    const instruction = await program.methods
      .increaseLiquidity(
        amount0In,
        amount1In,
        returnMint,
        tickLower,
        tickUpper,
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
            units: 450_000
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
    const transactionResult = await connection.simulateTransaction(transaction, {
      sigVerify: false,
      replaceRecentBlockhash: true,
      innerInstructions: true
    });

    expect(transactionResult.value.err).toBeNull();
  }, 500000);
});
