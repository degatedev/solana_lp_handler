import * as anchor from '@coral-xyz/anchor';
import { BN } from 'bn.js';
import {
  AddressLookupTableProgram,
  ComputeBudgetProgram,
  Keypair,
  PublicKey,
  SYSVAR_RENT_PUBKEY,
  Transaction,
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
  PositionInfoLayout,
  Raydium,
  SYSTEM_PROGRAM_ID,
  TickUtils
} from '@raydium-io/raydium-sdk-v2';
import { TOKEN_PROGRAM_ID } from '@coral-xyz/anchor/dist/cjs/utils/token';
import { ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token';
import { solveZapSingleSidedCLMM } from './utils';
import { loadPosition, openPositionForTest } from './fixtures';
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
  securityConfig,
  slippage,
  getFeeOwnerBalance,
  submitTransaction,
  wsolAtaExists,
  user,
  userWallet
} from './help';

// 环境变量已在 tests/setup.ts 中配置
// 如果需要覆盖，可以在这里设置：
// process.env.ANCHOR_PROVIDER_URL = "your-custom-rpc-url";
// process.env.ANCHOR_WALLET = "your-wallet-path";

describe('lp_withdraw', () => {
  // Configure the client to use the local cluster.

  let poolKeys: ClmmKeys;
  let raydium: Raydium;
  let positionNftMint: PublicKey;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
    // 现开一个仓供本用例减仓（模拟链上无法按 owner 枚举已有 position）
    positionNftMint = await openPositionForTest();
  }, 500000);
  it('lp_withdraw test', async () => {
    const poolProgramId = new PublicKey(poolKeys.programId);
    const poolInfo = await getPoolInfo();
    const position = await loadPosition(positionNftMint);
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

    const instruction = await program.methods
      .decreaseLiquidity(
        position.liquidity,
        new BN(0),
        new BN(0),
        deposit_token_mint,
        new BN(data.computePoolInfo.sqrtPriceX64.toString()),
        slippage,
        fee_percent,
        true
      )
      .accountsStrict({
        raydiumClmmProgram: CLMM_PROGRAM_ID,
        signer: user,
        recipient: user,
        ammConfig: new PublicKey(poolKeys.config.id),
        poolState: pool_address,
        observationState: new PublicKey(poolKeys.observationId),
        signerToken0Account: userToken0Account.tokenAccount,
        signerToken1Account: userToken1Account.tokenAccount,
        recipientToken0Account: userToken0Account.tokenAccount,
        recipientToken1Account: userToken1Account.tokenAccount,
        feeOwner: fee_address,
        positionNftAccount: positionNftAccount.publicKey,
        positionNftMint: position.nftMint,
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
        memoProgram: MEMO_PROGRAM_ID,
        securityConfig
      })
      .remainingAccounts(remainingAccounts)
      .instruction();
    // 注：传入 liquidity == position.liquidity 时，decrease_liquidity 内部会自动 CPI close_position，
    // 因此不再需要前端附加 ClmmInstrument.closePositionInstructions。

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
          ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }),
          ComputeBudgetProgram.setComputeUnitLimit({
            units: 450_000
          }),
          ComputeBudgetProgram.setComputeUnitPrice({
            microLamports: 1000
          }),
          userToken0Account.instruction,
          userToken1Account.instruction,
          feeToken0Account.instruction,
          feeToken1Account.instruction,
          ...createAccountsInstructions,
          instruction
        ]
      }).compileToV0Message(addressLookupTableAccounts)
    );
    const feeBefore = await getFeeOwnerBalance(deposit_token_mint);
    const transactionResult = await submitTransaction(transaction);
    expect(transactionResult.value.err).toBeNull();

    // DG-235：目标币为 USDC 时 wSOL 只作为中转，减仓结束后该 ATA 必须被关闭，
    // 否则会留下一个空账户长期占用用户约 0.002 SOL 租金。
    expect(await wsolAtaExists()).toBe(false);

    // 手续费只对 reward 抽成、不碰本金：本用例是"刚开仓即全额减仓"，
    // 期间没有累积任何 reward，因此 fee_owner 余额必须分文未增。
    // 这条断言同时守住 DG-235 的改动没有把本金误算进抽成口径。
    const feeAfter = await getFeeOwnerBalance(deposit_token_mint);
    expect(feeAfter).toBe(feeBefore);
  }, 5000000);
});
