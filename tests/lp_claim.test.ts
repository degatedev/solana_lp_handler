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
  ClmmInstrument,
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
import { TOKEN_PROGRAM_ID } from '@coral-xyz/anchor/dist/cjs/utils/token';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token';
import { solveZapSingleSidedCLMM } from './utils';
import {
  connection,
  deposit_amount,
  deposit_token_mint,
  fee_address,
  fee_percent,
  getPoolInfo,
  getPoolKeys,
  getRaydium,
  getTickArray,
  getTickLowerAndUpper,
  getTokenAta,
  pool_address,
  program,
  slippage,
  user,
  userWallet
} from './help';

// 环境变量已在 tests/setup.ts 中配置
// 如果需要覆盖，可以在这里设置：
// process.env.ANCHOR_PROVIDER_URL = "your-custom-rpc-url";
// process.env.ANCHOR_WALLET = "your-wallet-path";

const nft_mint = '';
describe('lp_claim', () => {
  // Configure the client to use the local cluster.

  let poolKeys: ClmmKeys;
  let raydium: Raydium;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
  });
  it('lp_claim test', async () => {
    const poolProgramId = new PublicKey(poolKeys.programId);
    const allPosition = await raydium.clmm.getOwnerPositionInfo({ programId: CLMM_PROGRAM_ID });
    const poolInfo = await getPoolInfo();
    const position = allPosition[0];
    console.log('position', allPosition.length, position.nftMint.toBase58(), userWallet.publicKey.toBase58());
    const { tickArrayLower, tickArrayUpper } = getTickArray(
      position.tickLower,
      position.tickUpper,
      poolKeys,
      poolProgramId
    );
    const [userToken0Account, userToken1Account, feeToken0Account, feeToken1Account] = await Promise.all([
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), user),
      getTokenAta(connection, new PublicKey(poolKeys.mintA.address), fee_address, user),
      getTokenAta(connection, new PublicKey(poolKeys.mintB.address), fee_address, user)
    ]);
    const positionNftAccount = getATAAddress(user, position.nftMint, TOKEN_2022_PROGRAM_ID);
    const protocolPosition = getPdaProtocolPositionAddress(
      poolProgramId,
      pool_address,
      position.tickLower,
      position.tickUpper
    ).publicKey;
    const personalPosition = getPdaPersonalPositionAddress(poolProgramId, position.nftMint);
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
      tokenOut: poolInfo[deposit_token_mint.equals(new PublicKey(poolKeys.mintA.address)) ? 'mintA' : 'mintB'],
      slippage: 0.01,
      epochInfo: await raydium.fetchEpochInfo()
    });

    // 添加 swap_remaining
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
    // 添加分隔符
    remainingAccounts.push({
      pubkey: program.programId,
      isSigner: false,
      isWritable: false
    });
    // 添加 decrease_liquidity_v2 的 remaining accounts

    const tickArrayLowerStartIndex = TickUtils.getTickArrayStartIndexByTick(
      position.tickLower,
      poolInfo.config.tickSpacing
    );
    const tickArrayUpperStartIndex = TickUtils.getTickArrayStartIndexByTick(
      position.tickUpper,
      poolInfo.config.tickSpacing
    );

    if (
      PoolUtils.isOverflowDefaultTickarrayBitmap(poolInfo.config.tickSpacing, [
        tickArrayLowerStartIndex,
        tickArrayUpperStartIndex
      ])
    ) {
      remainingAccounts.push({
        pubkey: tickArrayBitmapExtension,
        isSigner: false,
        isWritable: true
      });
    }
    const createAccountsInstructions = [];
    await Promise.all(
      poolInfo.rewardDefaultInfos.map(async (item, index) => {
        const mintAddress = item.mint.address;
        const ownerRewardVault = await getTokenAta(connection, new PublicKey(mintAddress), user);
        createAccountsInstructions.push(ownerRewardVault.instruction);
        const poolRewardVault = new PublicKey(poolKeys.rewardInfos[index].vault);
        const rewardMint = new PublicKey(mintAddress);
        remainingAccounts.push(
          {
            pubkey: poolRewardVault,
            isSigner: false,
            isWritable: true
          },
          {
            pubkey: ownerRewardVault.tokenAccount,
            isSigner: false,
            isWritable: true
          },
          {
            pubkey: rewardMint,
            isSigner: false,
            isWritable: true
          }
        );
      })
    );

    const accounts = {
      raydiumClmmProgram: CLMM_PROGRAM_ID,
      user: user,
      ammConfig: new PublicKey(poolKeys.config.id),
      poolState: pool_address,
      observationState: new PublicKey(poolKeys.observationId),
      userToken0Account: userToken0Account.tokenAccount,
      userToken1Account: userToken1Account.tokenAccount,
      feeOwner: fee_address,
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
      feeToken0Account: feeToken0Account.tokenAccount,
      feeToken1Account: feeToken1Account.tokenAccount,
      memoProgram: MEMO_PROGRAM_ID
    };
    console.log('accounts', JSON.stringify(accounts, null, 2));
    const instruction = await program.methods
      .decreaseLiquidity(new BN(0), new BN(0), new BN(0), deposit_token_mint, slippage, fee_percent, true)
      .accountsStrict(accounts)
      .remainingAccounts(remainingAccounts)
      .instruction();

    let addressLookupTableAccounts = [];
    const res = await connection.getAddressLookupTable(new PublicKey(poolKeys.lookupTableAccount));
    if (res.value) {
      addressLookupTableAccounts.push(res.value);
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
          ...createAccountsInstructions,
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
    console.log('transactionResult', txResult, transactionResult.value.logs);
  }, 5000000);
});
