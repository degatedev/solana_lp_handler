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
  ClmmInstrument,
  ClmmKeys,
  DEVNET_PROGRAM_ID,
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
describe('lp_withdraw', () => {
  // Configure the client to use the local cluster.

  let poolKeys: ClmmKeys;
  let raydium: Raydium;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
  });
  it('lp_withdraw test', async () => {
    const poolProgramId = new PublicKey(poolKeys.programId);
    const allPosition = await raydium.clmm.getOwnerPositionInfo({ programId: DEVNET_PROGRAM_ID.CLMM_PROGRAM_ID }); // devnet:
    const poolInfo = await getPoolInfo();
    const position = allPosition[1];
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

    // if (tickArrayBitmapExtension) {
    //   remainingAccounts.push({
    //     pubkey: tickArrayBitmapExtension,
    //     isSigner: false,
    //     isWritable: true
    //   });
    // }
    swapAmountOut.remainingAccounts.forEach((item) => {
      remainingAccounts.push({
        pubkey: item,
        isSigner: false,
        isWritable: true
      });
    });
    const accounts = {
      raydiumClmmProgram: DEVNET_PROGRAM_ID.CLMM_PROGRAM_ID,
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
    const instruction = await program.methods
      .decreaseLiquidity(position.liquidity, new BN(0), new BN(0), deposit_token_mint, slippage, fee_percent, true)
      .accountsStrict(accounts)
      .remainingAccounts(remainingAccounts)
      .instruction();
    const closeInsInfo = await ClmmInstrument.closePositionInstructions({
      poolInfo,
      poolKeys,
      ownerInfo: { wallet: user },
      ownerPosition: position,
      nft2022: true
    });

    let addressLookupTableAccounts = [];
    const res = await connection.getAddressLookupTable(
      new PublicKey(poolKeys.lookupTableAccount || '7d6JyYAdBWyFNVB47ydVrkydkyZehHAQVYbUrzSsG8wr')
    );
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
          instruction,
          ...closeInsInfo.instructions
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

  test('createAndExtendALT', async () => {
    const recentSlot = await connection.getSlot('finalized');
    const [createIx, lookupTableAddress] = AddressLookupTableProgram.createLookupTable({
      authority: userWallet.publicKey,
      payer: userWallet.publicKey,
      recentSlot
    });

    const extendIx = AddressLookupTableProgram.extendLookupTable({
      payer: userWallet.publicKey,
      authority: userWallet.publicKey,
      lookupTable: lookupTableAddress,
      addresses: [
        'DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH',
        'FVu3DGFoAqvG9gqrVBWrncDUF5ytqpvGVQxEKLp55Dvd',
        'CD4aJtX11cqTCAc83nxSPkkh5JW2yjD6uwHeovjqQ1qu',
        'FXAXqgjNK6JVzVV2frumKTEuxC8hTEUhVTJTRhMMwLmM',
        'CyMppkidzzGxvuT6dGx92Uxk9AYSaFJFygdVBe2P1SkV',
        'HYfoHMdkuGziyB9ySRe7kHkJyXAT3amfgsJr4kbe5cY8',
        'E5R75rvU4TCV78NZ26vajj7YCdHSD2bMY1NQiQSABk7B',
        '8X35rQUK2u9hfn8rMPwwr6ZSEUhbmfDPEapp589XyoM1',
        '3DVbay3sBmXMcoTFPv1CpbBE9nLmrrh3VCmKbeQFPKSH',
        '6L5KCx3t2aMd7h1B84PBKuLCjXw4xXkKatkAts2NeSDE',
        '5v2S9uVSF7SB45NNAhK8N8K1ckV4AuKE1ccyVvzzhJvK',
        'BtzVx4fv1yXVd7XXHYSfPS6xVifWWNtVamAo2trVgC7x',
        'EsyZsLsH1GMwz7QDmeyXH2U387dL7xQcPQMEULhmDRcW',
        'SysvarRent111111111111111111111111111111111',
        '11111111111111111111111111111111',
        'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA',
        'ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL',
        'TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb',
        '96MgzmNvDpGmhuferdtHHZAbGevibNccGM8ojpbqypPi',
        'FGrU9Vrikb3rpW4g4R81pR25dpH9mQXEqpf3cP4C97S9',
        'So11111111111111111111111111111111111111112',
        'USDCoctVLVnvTXBEuP9s8hntucdJokbo17RwHuNXemT',
        '56D9TcGdMVET4KFgQyauoidubvH6vH88RtScACWKCiR1',
        'Fb4BUx2QjqKdBL12pBnx8y8HdyKgvbPWdPXYkt6yb9VB',
        'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr'
      ].map((item) => new PublicKey(item))
    });
    const tx = new Transaction().add(createIx, extendIx);
    tx.feePayer = userWallet.publicKey;
    tx.recentBlockhash = (await connection.getLatestBlockhash()).blockhash;

    const signed = await userWallet.signTransaction(tx);
    const sig = await connection.sendRawTransaction(signed.serialize());
    await connection.confirmTransaction(sig, 'confirmed');
    console.log('sig', sig);
  });
});
