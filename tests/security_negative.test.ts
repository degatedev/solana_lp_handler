import * as anchor from '@coral-xyz/anchor';
import { BN } from 'bn.js';
import { ComputeBudgetProgram, PublicKey, SYSVAR_RENT_PUBKEY, TransactionMessage, VersionedTransaction } from '@solana/web3.js';
import {
  CLMM_PROGRAM_ID,
  ClmmKeys,
  getATAAddress,
  getPdaExBitmapAccount,
  getPdaPersonalPositionAddress,
  getPdaProtocolPositionAddress,
  MEMO_PROGRAM_ID,
  PoolUtils,
  Raydium,
  SYSTEM_PROGRAM_ID,
  TickUtils
} from '@raydium-io/raydium-sdk-v2';
import { TOKEN_PROGRAM_ID } from '@coral-xyz/anchor/dist/cjs/utils/token';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token';
import { connection, deposit_token_mint, fee_address, fee_percent, getPoolInfo, getPoolKeys, getRaydium, getTickArray, program, slippage, user, userWallet, deposit_amount, pool_address, getTokenAta, securityConfig } from './help';

// 两个负向用例：
// 1) remaining_accounts 注入未允许的 executable program account => 安全层应拒绝
// 2) 注入两个分隔符（program.programId）=> 安全层应拒绝

const STAKE_PROGRAM_ID = new PublicKey('Stake11111111111111111111111111111111111111');

describe('security_negative', () => {
  let poolKeys: ClmmKeys;
  let raydium: Raydium;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
  }, 50000);

  async function buildDecreaseLiquidityIx(extraRemaining: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[]) {
    const poolProgramId = new PublicKey(poolKeys.programId);
    const allPosition = await raydium.clmm.getOwnerPositionInfo({ programId: CLMM_PROGRAM_ID });
    if (!allPosition.length) throw new Error('wallet has no CLMM positions; cannot run security_negative tests');
    const position = allPosition[0];
    const poolInfo = await getPoolInfo();

    const { tickArrayLower, tickArrayUpper } = getTickArray(position.tickLower, position.tickUpper, poolKeys, poolProgramId);
    const [userToken0Account, userToken1Account, feeToken0Account, feeToken1Account] = await Promise.all([
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), fee_address, user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), fee_address, user)
    ]);

    const positionNftAccount = getATAAddress(user, position.nftMint, TOKEN_2022_PROGRAM_ID);
    const protocolPosition = getPdaProtocolPositionAddress(poolProgramId, pool_address, position.tickLower, position.tickUpper).publicKey;
    const personalPosition = getPdaPersonalPositionAddress(poolProgramId, position.nftMint);
    const tickArrayBitmapExtension = getPdaExBitmapAccount(poolProgramId, pool_address).publicKey;

    const remainingAccounts: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[] = [];

    // swap_remaining（用于 decrease_liquidity 内部 convert_to_usdc 的 swap）
    const data = await raydium.clmm.getPoolInfoFromRpc(pool_address.toBase58());
    const tickArrayCache = data.tickData;
    const swapAmountOut = await PoolUtils.computeAmountOutFormat({
      poolInfo: data.computePoolInfo,
      tickArrayCache: tickArrayCache[pool_address.toBase58()],
      amountIn: new BN(deposit_amount),
      tokenOut: poolInfo[deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address)) ? 'mintA' : 'mintB'],
      slippage: 0.01,
      epochInfo: await raydium.fetchEpochInfo()
    });

    if (tickArrayBitmapExtension) {
      remainingAccounts.push({ pubkey: tickArrayBitmapExtension, isSigner: false, isWritable: true });
    }
    swapAmountOut.remainingAccounts.forEach((item) => {
      remainingAccounts.push({ pubkey: item, isSigner: false, isWritable: true });
    });

    // 分隔符（正常情况必须且只能出现一次）
    remainingAccounts.push({ pubkey: program.programId, isSigner: false, isWritable: false });

    // decrease_liquidity_v2 的 remaining accounts（奖励相关）
    const tickArrayLowerStartIndex = TickUtils.getTickArrayStartIndexByTick(position.tickLower, poolInfo.config.tickSpacing);
    const tickArrayUpperStartIndex = TickUtils.getTickArrayStartIndexByTick(position.tickUpper, poolInfo.config.tickSpacing);
    if (PoolUtils.isOverflowDefaultTickarrayBitmap(poolInfo.config.tickSpacing, [tickArrayLowerStartIndex, tickArrayUpperStartIndex])) {
      remainingAccounts.push({ pubkey: tickArrayBitmapExtension, isSigner: false, isWritable: true });
    }

    // 追加负向用例注入
    remainingAccounts.push(...extraRemaining);

    const accounts = {
      raydiumClmmProgram: CLMM_PROGRAM_ID,
      user: user,
      userToken0Account: userToken0Account.tokenAccount,
      userToken1Account: userToken1Account.tokenAccount,
      feeOwner: fee_address,
      feeToken0Account: feeToken0Account.tokenAccount,
      feeToken1Account: feeToken1Account.tokenAccount,
      ammConfig: new PublicKey(poolKeys.config.id),
      poolState: pool_address,
      observationState: new PublicKey(poolKeys.observationId),
      rent: SYSVAR_RENT_PUBKEY,
      systemProgram: SYSTEM_PROGRAM_ID,
      tokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      tokenProgram2022: TOKEN_2022_PROGRAM_ID,
      memoProgram: MEMO_PROGRAM_ID,
      tokenVault0: new PublicKey(poolKeys.vault.A),
      tokenVault1: new PublicKey(poolKeys.vault.B),
      vault0Mint: new PublicKey(poolKeys.mintA.address),
      vault1Mint: new PublicKey(poolKeys.mintB.address),
      positionNftAccount: positionNftAccount.publicKey,
      protocolPosition,
      personalPosition: personalPosition.publicKey,
      tickArrayLower,
      tickArrayUpper,
      securityConfig
    };

    return await program.methods
      .decreaseLiquidity(position.liquidity, new BN(0), new BN(0), deposit_token_mint, slippage, fee_percent, true)
      .accountsStrict(accounts)
      .remainingAccounts(remainingAccounts)
      .instruction();
  }

  it('rejects injected executable program account', async () => {
    const ix = await buildDecreaseLiquidityIx([{ pubkey: STAKE_PROGRAM_ID, isSigner: false, isWritable: false }]);
    const tx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: user,
        recentBlockhash: (await connection.getLatestBlockhash()).blockhash,
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({ units: 450_000 }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
          ix
        ]
      }).compileToV0Message()
    );
    userWallet.signTransaction(tx);
    const sim = await connection.simulateTransaction(tx, {
      sigVerify: false,
      replaceRecentBlockhash: true,
      innerInstructions: true
    });
    expect(sim.value.err).toBeTruthy();
  }, 200000);

  it('rejects duplicated separator account', async () => {
    // 再插入一次分隔符
    const ix = await buildDecreaseLiquidityIx([{ pubkey: program.programId, isSigner: false, isWritable: false }]);
    const tx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: user,
        recentBlockhash: (await connection.getLatestBlockhash()).blockhash,
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({ units: 450_000 }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
          ix
        ]
      }).compileToV0Message()
    );
    userWallet.signTransaction(tx);
    const sim = await connection.simulateTransaction(tx, {
      sigVerify: false,
      replaceRecentBlockhash: true,
      innerInstructions: true
    });
    expect(sim.value.err).toBeTruthy();
  }, 200000);
});

