import * as anchor from '@coral-xyz/anchor';
import { Program } from '@coral-xyz/anchor';
import { LpHandler } from '../target/types/lp_handler';
import {
  ApiV3PoolInfoConcentratedItem,
  ClmmKeys,
  getATAAddress,
  getPdaTickArrayAddress,
  Raydium,
  TickUtils
} from '@raydium-io/raydium-sdk-v2';
import { Connection, PublicKey } from '@solana/web3.js';
import { createAssociatedTokenAccountIdempotentInstruction, TOKEN_PROGRAM_ID } from '@solana/spl-token';
import Decimal from 'decimal.js';

anchor.setProvider(anchor.AnchorProvider.env());

export const program = anchor.workspace.lpHandler as Program<LpHandler>;

export const userWallet = anchor.AnchorProvider.env().wallet;
export const user = userWallet.publicKey;
export const pool_address = new PublicKey('FXAXqgjNK6JVzVV2frumKTEuxC8hTEUhVTJTRhMMwLmM');

export const connection = anchor.AnchorProvider.env().connection;

export const getRaydium = async () => {
  return await Raydium.load({
    connection: connection,
    cluster: 'devnet',
    disableFeatureCheck: true,
    owner: user,
    disableLoadToken: true,
    urlConfigs: {
      BASE_HOST: 'https://api-v3-devnet.raydium.io'
    }
  });
};

export const getPoolKeys = async () => {
  const raydium = await getRaydium();
  const p = await raydium.api.fetchPoolKeysById({ idList: [pool_address.toBase58()] });
  return p[0] as ClmmKeys;
};

export const getPoolInfo = async () => {
  const raydium = await getRaydium();
  const [poolInfo] = await raydium.api.fetchPoolById({ ids: pool_address.toBase58() });
  return poolInfo as unknown as ApiV3PoolInfoConcentratedItem;
};
export const getTokenAccountProgramId = async (connection: Connection, mint: PublicKey) => {
  const tokenAccountInfo = await connection.getAccountInfo(mint);
  return tokenAccountInfo?.owner ?? TOKEN_PROGRAM_ID;
};

export const getTokenAta = async (connection: Connection, mint: PublicKey, user: PublicKey, payer?: PublicKey) => {
  const programId = await getTokenAccountProgramId(connection, mint);
  const tokenAccount = getATAAddress(user, mint, programId).publicKey;
  const instruction = createAssociatedTokenAccountIdempotentInstruction(
    payer || user,
    tokenAccount,
    user,
    mint,
    programId
  );

  return {
    mint,
    tokenAccount,
    instruction,
    programId
  };
};

export const getTickLowerAndUpper = (poolKeys: ClmmKeys, startPrice: number, endPrice: number) => {
  const { tick: tick0 } = TickUtils.getPriceAndTick({
    poolInfo: poolKeys as unknown as ApiV3PoolInfoConcentratedItem,
    price: new Decimal(startPrice),
    baseIn: true
  });

  const { tick: tick1 } = TickUtils.getPriceAndTick({
    poolInfo: poolKeys as unknown as ApiV3PoolInfoConcentratedItem,
    price: new Decimal(endPrice),
    baseIn: true
  });
  const tickLower = Math.min(tick0, tick1);
  const tickUpper = Math.max(tick0, tick1);
  return { tickLower, tickUpper };
};

export const getTickArray = (tickLower: number, tickUpper: number, poolKeys: ClmmKeys, poolProgramId: PublicKey) => {
  const tickArrayLowerStartIndex = TickUtils.getTickArrayStartIndexByTick(tickLower, poolKeys.config.tickSpacing);
  const tickArrayLower = getPdaTickArrayAddress(poolProgramId, pool_address, tickArrayLowerStartIndex).publicKey;
  const tickArrayUpperStartIndex = TickUtils.getTickArrayStartIndexByTick(tickUpper, poolKeys.config.tickSpacing);
  const tickArrayUpper = getPdaTickArrayAddress(poolProgramId, pool_address, tickArrayUpperStartIndex).publicKey;
  return { tickArrayLower, tickArrayUpper };
};

export const deposit_amount = 111111111;
export const deposit_token_mint = new PublicKey('So11111111111111111111111111111111111111112');
export const slippage = 5000;
export const startPrice = 100;
export const endPrice = 200;
export const fee_address = new PublicKey('8X35rQUK2u9hfn8rMPwwr6ZSEUhbmfDPEapp589XyoM1');
export const fee_percent = 1200;
