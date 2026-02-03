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
export const pool_address = new PublicKey('3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv');

export const connection = anchor.AnchorProvider.env().connection;

export const getRaydium = async () => {
  return await Raydium.load({
    connection: connection,
    cluster: 'mainnet',
    disableFeatureCheck: true,
    owner: user,
    disableLoadToken: true
  });
};

export const getPoolKeys = async (poolId: PublicKey = pool_address) => {
  const raydium = await getRaydium();
  const p = await raydium.api.fetchPoolKeysById({ idList: [poolId.toBase58()] });
  return p[0] as ClmmKeys;
};

export const getPoolInfo = async (poolId: PublicKey = pool_address) => {
  const raydium = await getRaydium();
  const [poolInfo] = await raydium.api.fetchPoolById({ ids: poolId.toBase58() });
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

export const deposit_amount = 1000000;
export const deposit_token_mint = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
export const slippage = 50;
export const startPrice = 431;
export const endPrice = 522;
export const fee_address = new PublicKey('E32ykUTbi4Ag8t4Hic41HtDVwAZca1oGorqvkt3YS7Dy');
export const fee_percent = 1200;

export const securityConfig = PublicKey.findProgramAddressSync([Buffer.from('security_config')], program.programId)[0]



export const pools =[
  new PublicKey('HHQUnUbmWLrYzkscDY1C3deEFbGtiGBGoHjpANogmvum'), // TSLAx-USDC
  new PublicKey('CKwJZwm7oj3nu4653N1EpDrqXbXAYXoPFiPeEnLouF8y') ,//AAPLx-USDC
  new PublicKey('6m5aXAve4uh6Kt4ytKyCLWNMjd8PYP5vujwNCtycrUiD'),//AMZNx-USDC
  new PublicKey('4KqQN6u1pFKroFE2jVEhoepAMRKPcuAzWVDCgm9zRBYN'), // NVDAx
  new PublicKey('3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv'), // SOL-USDC
  new PublicKey('G39wywquKbHK8F2wZZZFX3fcsyG91VCCbbr6WEVp5axy'), // CRCLx-USDC
  new PublicKey('RyhF4cksVZY7vcqJpoytHcxcGNKRp27PEGhSnEPpbGv'), // MSTRx-USDC
  new PublicKey('7sHMnvE7WqP7vQFWJGEnMT4vZg6Za9K7PpddDoXJCqME'), // SPYx-USDC
  new PublicKey('B8YAwjGYk6qidWzGBXMAxP7nYfG8g74EZ3Y4gFSsobRw'), // GOOGLx
  new PublicKey('FknDV1F5n6QaA7rLmjquDjuU6wcPMNm5RYq7zWbqhpZw'), // QQQx-USDC
  new PublicKey('3L7KbPVaAQA4UTecaGQYsm6UCq5F3sZM9zAYkxqYt63j'), // Meta-USDC
  new PublicKey('FknDV1F5n6QaA7rLmjquDjuU6wcPMNm5RYq7zWbqhpZw'), // QQQx-USDC
]