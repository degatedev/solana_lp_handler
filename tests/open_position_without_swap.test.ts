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
import { openPositionWithoutSwap } from './fixtures';
import {
  connection,
  deposit_amount,
  deposit_token_mint,
  fee_address,
  getPoolInfo,
  getPoolKeys,
  getRaydium,
  getTickArray,
  ensureFunds,
  getPriceRange,
  getTickLowerAndUpper,
  getTokenAta,
  pool_address,
  program,
  securityConfig,
  slippage,
  submitTransaction,
  user
} from './help';

describe('open_position_without_swap', () => {
  let poolKeys: ClmmKeys;
  let raydium: Raydium;

  beforeAll(async () => {
    raydium = await getRaydium();
    poolKeys = await getPoolKeys();
    await ensureFunds();
  }, 50000);

  it('open position without swap succeeds', async () => {
    const { transactionResult } = await openPositionWithoutSwap();
    expect(transactionResult.value.err).toBeNull();
  }, 500000);

  it('rejects wildly wrong quoted liquidity', async () => {
    const { computedLiquidity } = await openPositionWithoutSwap();
    // 传入 2 倍 liquidity，slippage=0 → LiquiditySlippageExceeded
    const { transactionResult } = await openPositionWithoutSwap({
      liquidityOverride: computedLiquidity.muln(2),
      slippageBps: 0
    });

    expect(transactionResult.value.err).not.toBeNull();
    expect((transactionResult.value.logs || []).join('\n')).toContain('LiquiditySlippageExceeded');
  }, 500000);
});
