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
import { solveZapSingleSidedCLMM } from './utils';
import {
  connection,
  deposit_amount,
  deposit_token_mint,
  endPrice,
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
    const [userToken0Account, userToken1Account] = await Promise.all([
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user)
    ]);

    // const metadataAccount = getPdaMetadataKey(positionNftMint);

    const positionNftAccount = getATAAddress(user, position.nftMint, TOKEN_2022_PROGRAM_ID);
    const personalPosition = getPdaPersonalPositionAddress(poolProgramId, position.nftMint);
    const poolInfo = await getPoolInfo();
    const res = await solveZapSingleSidedCLMM({
      connection: raydium.connection,
      apiPoolItem: poolInfo,
      tickLower: tickLower,
      tickUpper: tickUpper,
      inputMint: deposit_token_mint.toBase58(),
      amountInBN: new BN(deposit_amount),
      slippage: 0
    });

    let amount = new BN(deposit_amount).sub(res.swapAmountIN);
    let inputA = deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address));
    if (amount.eq(new BN(0))) {
      inputA = false;
      amount = res.swapAmountOut;
    }

    const epochInfo = await raydium.fetchEpochInfo();
    const res2 = await PoolUtils.getLiquidityAmountOutFromAmountIn({
      poolInfo: poolInfo as unknown as ApiV3PoolInfoConcentratedItem,
      slippage: 0,
      inputA,
      tickUpper,
      tickLower,
      amount,
      add: true,
      amountHasFee: true,
      epochInfo: epochInfo
    });
    console.log('res', res2.amountA.amount.toString(), res2.amountB.amount.toString());
    const tickArrayBitmapExtension = getPdaExBitmapAccount(poolProgramId, pool_address).publicKey;
    const remainingAccounts = [];

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
      amountIn: new BN(deposit_amount),
      tokenOut: poolInfo[deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address)) ? 'mintB' : 'mintA'],
      slippage: 0.01,
      epochInfo: await raydium.fetchEpochInfo()
    });
    if (tickArrayBitmapExtension) {
      remainingAccounts.push({
        pubkey: tickArrayBitmapExtension,
        isSigner: false,
        isWritable: true
      });
    }
    swapAmountOut.remainingAccounts.forEach((item) => {
      remainingAccounts.push({
        pubkey: item,
        isSigner: false,
        isWritable: true
      });
    });
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

  
    const amount0In = deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address)) ? new BN(deposit_amount) : new BN(0);
    const amount1In = deposit_token_mint.equals(new PublicKey(poolKeys.mintB.address)) ? new BN(deposit_amount) : new BN(0);
    const instruction = await program.methods
      .increaseLiquidity(amount0In, amount1In, deposit_token_mint, tickLower, tickUpper, slippage, slippage)
      .accountsStrict({
        raydiumClmmProgram: CLMM_PROGRAM_ID,
        memoProgram: MEMO_PROGRAM_ID,
        user: user,
        ammConfig: new PublicKey(poolKeys.config.id),
        poolState: pool_address,
        observationState: new PublicKey(poolKeys.observationId),
        userToken0Account: userToken0Account.tokenAccount,
        userToken1Account: userToken1Account.tokenAccount,
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

    userWallet.signTransaction(transaction);
    const txResult = await connection.sendRawTransaction(transaction.serialize(), {
      skipPreflight: false
    });
    console.log('txResult', txResult);
    console.log('transactionResult', transactionResult.value.logs);
  }, 500000);

  it.skip('two-sided input example (manual funding required)', async () => {
    // 说明：这是“双币输入”接口示例。
    // 运行该用例前，需要确保 user 同时持有 pool 的 token0/token1，并准备好对应 ATA。
    await program.methods.increaseLiquidity(new BN(1), new BN(1), deposit_token_mint, 0, 1, slippage, slippage);
  });

  test('parse log test', async () => {
    // 498PU5rrcysb6vaL77DRRfpiF296in484oPqWcNyZTgBvhaa5djiHXLhXHtYaRp35d6AaeduPpbkNrr7nKYfcHMG

    const data = await connection.getParsedTransaction(
      '3U3CDUMLqf1We1Cq9ULVnjRzk1x7waXJk1jrYSGTr9FfW1ePvRY5zAGwdNAp5tbyAD1YXMZyNNsxTZjTAeWkQbzq',
      {
        maxSupportedTransactionVersion: 1,
        commitment: 'confirmed'
      }
    );

    const program1 = new Program(amm_v3, { connection });
    const parser1 = new anchor.EventParser(program1.programId, program.coder);
    const events1 = parser1.parseLogs(data?.meta?.logMessages || []);
    const parser = new anchor.EventParser(program.programId, program.coder);
    const events = parser.parseLogs(data.meta?.logMessages || []);
    const list: any[] = [];
    for (const obj of events) {
      list.push({ name: obj.name, data: obj.data });
    }
    console.log('list', list);
    const list2: any[] = [];
    for (const obj of events1) {
      list2.push({ name: obj.name, data: obj.data });
    }
    console.log('list2', list2);
  });

  // test('test', async () => {
  //   const ata = await getTokenAta(connection, new PublicKey('So11111111111111111111111111111111111111112'), caller);
  //   const tx = new Transaction().add(
  //     ata.instruction,
  //     SystemProgram.transfer({
  //       fromPubkey: caller,
  //       toPubkey: ata.tokenAccount,
  //       lamports: 2 * 1e9 // 0.5 SOL
  //     }),
  //     // 3) 同步，使其变成 WSOL 余额
  //     createSyncNativeInstruction(ata.tokenAccount)
  //   );
  //   tx.feePayer = caller;
  //   tx.recentBlockhash = (await connection.getLatestBlockhash()).blockhash;
  //   userWallet.signTransaction(tx);

  //   const txResult = await connection.sendRawTransaction(tx.serialize(), {
  //     skipPreflight: true
  //   });
  //   console.log('txResult', txResult);
  // });
});
