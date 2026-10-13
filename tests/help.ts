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
import {
  ComputeBudgetProgram,
  Connection,
  PublicKey,
  Signer,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction
} from '@solana/web3.js';
import { createAssociatedTokenAccountIdempotentInstruction, TOKEN_PROGRAM_ID } from '@solana/spl-token';
import Decimal from 'decimal.js';
import * as os from 'os';
import * as path from 'path';

// ============================================================================
// 运行环境：默认跑模拟链（chain_simulator 的 Solana fork），不碰主网。
// 设置 USE_MAINNET=1 才会使用 .env 里的 ANCHOR_PROVIDER_URL 连真实主网。
// ============================================================================
const SIMULATOR_BASE = process.env.SIMULATOR_BASE_URL || 'https://chain-simulator.dev.degate.tech/chain_simulator/501';

export const isSimulator = process.env.USE_MAINNET !== '1';

if (isSimulator) {
  process.env.ANCHOR_PROVIDER_URL = `${SIMULATOR_BASE}/rpc`;
}

// .env 里的 ANCHOR_WALLET 可能写成 ~/...，dotenv 不做 tilde 展开
if (process.env.ANCHOR_WALLET?.startsWith('~')) {
  process.env.ANCHOR_WALLET = path.join(os.homedir(), process.env.ANCHOR_WALLET.slice(1));
}

anchor.setProvider(anchor.AnchorProvider.env());

export const program = anchor.workspace.lpHandler as Program<LpHandler>;

export const userWallet = anchor.AnchorProvider.env().wallet;
export const user = userWallet.publicKey;
export const pool_address = new PublicKey('3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv');

export const connection = anchor.AnchorProvider.env().connection;

// 发送/确认交易要求 commitment 至少为 confirmed，而 AnchorProvider 默认是 processed
const confirmedConnection = new Connection(connection.rpcEndpoint, 'confirmed');

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
export const fee_address = new PublicKey('J3t7ucPyFEh4QH7ut1nfLe1JKC18NU476tyhxNsQvnqg');
export const fee_percent = 1200;

export const securityConfig = PublicKey.findProgramAddressSync([Buffer.from('security_config')], program.programId)[0];

export const pools = [
  new PublicKey('HHQUnUbmWLrYzkscDY1C3deEFbGtiGBGoHjpANogmvum'), // TSLAx-USDC
  new PublicKey('CKwJZwm7oj3nu4653N1EpDrqXbXAYXoPFiPeEnLouF8y'), //AAPLx-USDC
  new PublicKey('6m5aXAve4uh6Kt4ytKyCLWNMjd8PYP5vujwNCtycrUiD'), //AMZNx-USDC
  new PublicKey('4KqQN6u1pFKroFE2jVEhoepAMRKPcuAzWVDCgm9zRBYN'), // NVDAx
  new PublicKey('3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv'), // SOL-USDC
  new PublicKey('G39wywquKbHK8F2wZZZFX3fcsyG91VCCbbr6WEVp5axy'), // CRCLx-USDC
  new PublicKey('RyhF4cksVZY7vcqJpoytHcxcGNKRp27PEGhSnEPpbGv'), // MSTRx-USDC
  new PublicKey('7sHMnvE7WqP7vQFWJGEnMT4vZg6Za9K7PpddDoXJCqME'), // SPYx-USDC
  new PublicKey('B8YAwjGYk6qidWzGBXMAxP7nYfG8g74EZ3Y4gFSsobRw'), // GOOGLx
  new PublicKey('FknDV1F5n6QaA7rLmjquDjuU6wcPMNm5RYq7zWbqhpZw'), // QQQx-USDC
  new PublicKey('3L7KbPVaAQA4UTecaGQYsm6UCq5F3sZM9zAYkxqYt63j'), // Meta-USDC
  new PublicKey('FknDV1F5n6QaA7rLmjquDjuU6wcPMNm5RYq7zWbqhpZw') // QQQx-USDC
];

// ============================================================================
// 模拟链辅助：资金准备 / 交易提交 / 动态价格区间
// ============================================================================

/// 给测试钱包充值。仅在模拟链生效，主网上是 no-op。
export const ensureFunds = async (sol = 200, usdc = 100000) => {
  if (!isSimulator) return;
  const addr = user.toBase58();
  await fetch(`${SIMULATOR_BASE}/fund/${addr}?amount=${sol}`, { method: 'POST' });
  await fetch(`${SIMULATOR_BASE}/fund/${addr}/${deposit_token_mint.toBase58()}?amount=${usdc}&decimals=6`, {
    method: 'POST'
  });
};

export type SubmitResult = {
  value: { err: unknown; logs: string[] };
  signature?: string;
};

/// 统一的交易提交入口。
/// - 模拟链：真实发送并确认，返回落账后的 err/logs（这样才能验证账户的最终状态）
/// - 主网：退回 simulateTransaction，绝不真实发送
export const submitTransaction = async (
  transaction: VersionedTransaction,
  extraSigners: Signer[] = []
): Promise<SubmitResult> => {
  if (!isSimulator) {
    const sim = await connection.simulateTransaction(transaction, {
      sigVerify: false,
      replaceRecentBlockhash: true,
      innerInstructions: true
    });
    return { value: { err: sim.value.err, logs: sim.value.logs ?? [] } };
  }

  if (extraSigners.length > 0) transaction.sign(extraSigners);
  const signed = (await userWallet.signTransaction(transaction)) as VersionedTransaction;

  try {
    const signature = await confirmedConnection.sendTransaction(signed, { skipPreflight: false });
    const bh = await confirmedConnection.getLatestBlockhash();
    await confirmedConnection.confirmTransaction({ signature, ...bh }, 'confirmed');
    const tx = await confirmedConnection.getTransaction(signature, { maxSupportedTransactionVersion: 0 });
    return { value: { err: tx?.meta?.err ?? null, logs: tx?.meta?.logMessages ?? [] }, signature };
  } catch (e: any) {
    // 交易被节点拒绝：把 err/logs 还原成与 simulate 一致的形状，供负向用例断言
    const logs: string[] = e?.logs ?? (typeof e?.getLogs === 'function' ? (await e.getLogs()) ?? [] : []);
    return { value: { err: e?.transactionError?.err ?? e?.message ?? 'TransactionError', logs } };
  }
};

/// 按当前池价动态生成价格区间。写死的区间会随行情漂移失效，因此一律按现价取。
///
/// - `straddle`（默认）：区间跨越现价，两种币都需要，适合 zap（带 swap）场景
/// - `belowSpot`：区间完全低于现价，该区间的流动性只由 quote 币（token1 / USDC）提供，
///   适合"不做 swap、只投单边 USDC"的场景 —— 跨现价的区间下单边入金算出的
///   liquidity 是 min(L0, L1) = 0，会直接被链上的 `quoted > 0` 拒绝。
export const getPriceRange = async (mode: 'straddle' | 'belowSpot' = 'straddle', poolId: PublicKey = pool_address) => {
  const info = await getPoolInfo(poolId);
  const price = Number(info.price);
  return mode === 'belowSpot'
    ? { startPrice: price * 0.5, endPrice: price * 0.8 }
    : { startPrice: price * 0.8, endPrice: price * 1.25 };
};

/// signer 的 wSOL ATA 地址。全换单币场景下它只作为中转，指令结束时应当被关闭（DG-235）。
export const getSignerWsolAta = () =>
  getATAAddress(user, new PublicKey('So11111111111111111111111111111111111111112'), TOKEN_PROGRAM_ID).publicKey;

export const wsolAtaExists = async () => (await connection.getAccountInfo(getSignerWsolAta())) !== null;

/// 读取 fee_owner 在某个 mint 上的余额（账户不存在时按 0 处理），用于校验手续费确实入账。
export const getFeeOwnerBalance = async (mint: PublicKey): Promise<bigint> => {
  const programId = await getTokenAccountProgramId(connection, mint);
  const ata = getATAAddress(fee_address, mint, programId).publicKey;
  const info = await connection.getTokenAccountBalance(ata).catch(() => null);
  return info ? BigInt(info.value.amount) : 0n;
};

/// 把模拟链上的某个账户重置为"未初始化"（system-owned、data 为空）。仅模拟链可用。
export const resetSimAccount = async (address: PublicKey) => {
  if (!isSimulator) throw new Error('resetSimAccount 仅能在模拟链上使用');
  await fetch(`${SIMULATOR_BASE}/rpc`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'surfnet_setAccount',
      params: [
        address.toBase58(),
        { lamports: 0, data: '', owner: '11111111111111111111111111111111', executable: false, rentEpoch: 0 }
      ]
    })
  });
};

/// 确保 SecurityConfig PDA 存在且白名单为测试基准值（pools + fee_address）。
/// security_config 用例会重置/关闭这个 PDA，跑完必须恢复，否则其他用例的白名单校验会被跳过。
export const ensureSecurityConfig = async () => {
  const existing = await connection.getAccountInfo(securityConfig);
  if (existing && existing.data.length > 0) return;

  const ix = await program.methods
    .initSecurityConfig(pools, [fee_address])
    .accountsStrict({
      authority: user,
      securityConfig,
      systemProgram: SystemProgram.programId
    })
    .instruction();

  const tx = new VersionedTransaction(
    new TransactionMessage({
      payerKey: user,
      recentBlockhash: (await connection.getLatestBlockhash()).blockhash,
      instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }), ix]
    }).compileToV0Message()
  );

  const res = await submitTransaction(tx);
  if (res.value.err !== null) {
    throw new Error('ensureSecurityConfig failed: ' + JSON.stringify(res.value.err));
  }
};
