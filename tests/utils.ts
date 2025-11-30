// clmm-zap-single-sided.ts
import BN from 'bn.js';
import Decimal from 'decimal.js';
import { Connection, EpochInfo } from '@solana/web3.js';
import {
  ApiV3PoolInfoConcentratedItem,
  ApiV3Token,
  ComputeClmmPoolInfo,
  ReturnTypeFetchMultiplePoolTickArrays,
  PoolUtils,
  SqrtPriceMath,
  Q64
} from '@raydium-io/raydium-sdk-v2';

/** —— 工具 —— */
const bnToDec = (bn: BN) => new Decimal(bn.toString());
const decToBn = (x: Decimal) => new BN(x.toFixed(0));
const sFromX64 = (x64: BN) => new Decimal(x64.toString()).div(new Decimal(Q64.toString()));

/** 构建离链报价上下文（含 tick 缓存与 epochInfo） */
export async function buildClmmQuoteContext(opts: {
  connection: Connection;
  apiPoolItem: Pick<ApiV3PoolInfoConcentratedItem, 'id' | 'programId' | 'mintA' | 'mintB' | 'config' | 'price'>;
  batchRequest?: boolean;
}) {
  const { connection, apiPoolItem, batchRequest = true } = opts;

  const computePool: ComputeClmmPoolInfo = await PoolUtils.fetchComputeClmmInfo({
    connection,
    poolInfo: apiPoolItem
  });

  const tickArrayCache: ReturnTypeFetchMultiplePoolTickArrays = await PoolUtils.fetchMultiplePoolTickArrays({
    connection,
    poolKeys: [computePool],
    batchRequest
  });

  const epochInfo: EpochInfo = await connection.getEpochInfo();

  /** 通用报价：给定输入边与金额，返回另一边输出与 allTrade */
  const quote = (amountIn: BN, inputIsMintA: boolean, slippage = 0): { out: BN; allTrade: boolean; minOut: BN } => {
    if (amountIn.lten(0)) {
      return { out: new BN(0), allTrade: true, minOut: new BN(0) };
    }

    const tokenOut: ApiV3Token = inputIsMintA ? (apiPoolItem.mintB as any) : (apiPoolItem.mintA as any);
    const res = PoolUtils.computeAmountOutFormat({
      poolInfo: computePool,
      tickArrayCache: tickArrayCache[computePool.id.toBase58()],
      amountIn,
      tokenOut,
      slippage, // 报价用，slippage=0
      epochInfo,
      catchLiquidityInsufficient: true // 覆盖不足时返回 allTrade=false
    });
    return {
      out: res.amountOut.amount.raw.sub(new BN(1)),
      allTrade: res.allTrade,
      minOut: res.minAmountOut.amount.raw
    };
  };

  return { computePool, tickArrayCache, epochInfo, quote };
}

/** 通用单边最优拆分（任意池、任意输入边） */
export async function solveZapSingleSidedCLMM(opts: {
  connection: Connection;
  apiPoolItem: Pick<ApiV3PoolInfoConcentratedItem, 'id' | 'programId' | 'mintA' | 'mintB' | 'config' | 'price'>;
  tickLower: number;
  tickUpper: number;

  /** 单边输入：指定哪一边（必须等于 mintA/mintB 的 address）与金额（最小单位） */
  inputMint: string; // = pool.mintA.address 或 pool.mintB.address
  amountInBN: BN;

  slippage?: number;
  /** 二分搜索控制参数 */
  ratioTolerance?: number; // 目标比值相对误差阈值（默认 1e-8）
  amountToleranceBN?: BN; // 金额步长阈值（默认 ≈ 1e-6 * 10^decimals(input)）
  maxIter?: number; // 最大迭代次数（默认 60）
}) {
  const {
    connection,
    apiPoolItem,
    tickLower,
    tickUpper,
    inputMint,
    amountInBN,
    ratioTolerance = 1e-8,
    amountToleranceBN,
    slippage,
    maxIter = 100
  } = opts;

  const inputIsMintA = inputMint === apiPoolItem.mintA.address;
  if (!inputIsMintA && inputMint !== apiPoolItem.mintB.address) {
    throw new Error('inputMint 必须等于 pool.mintA.address 或 pool.mintB.address');
  }
  const { computePool, quote } = await buildClmmQuoteContext({ connection, apiPoolItem });

  // —— 价格边界（使用链上 √P，避免方向错误）——
  const sa = sFromX64(SqrtPriceMath.getSqrtPriceX64FromTick(tickLower));
  const sb = sFromX64(SqrtPriceMath.getSqrtPriceX64FromTick(tickUpper));
  const sp = sFromX64(computePool.sqrtPriceX64);

  // —— 区间不跨现价：只需单边 —— //
  if (sp.lte(sa) || sp.gte(sb)) {
    const onlyToken0 = sp.lte(sa); // true=仅需 token0(mintA)，false=仅需 token1(mintB)

    // 若输入不是需要的那边，则“全额换”；否则无需换
    let swapAmountIN = new BN(0);
    let swapAmountOut = new BN(0);
    let keepInputBN = amountInBN;
    let note = onlyToken0 ? '区间在现价上方，仅需 token0' : '区间在现价下方，仅需 token1';
    let allTradeOk = true;

    if ((onlyToken0 && !inputIsMintA) || (!onlyToken0 && inputIsMintA)) {
      // 需要把全部换成目标边
      const quoted = quote(amountInBN, inputIsMintA, slippage);
      swapAmountIN = amountInBN;
      swapAmountOut = quoted.minOut;
      keepInputBN = new BN(0);
      allTradeOk = quoted.allTrade;
      if (!allTradeOk) note += '（警告：报价未完全成交，tick 缓存不足）';
    }

    return {
      mode: 'single-token-only' as const,
      note,
      swapAmountIN,
      swapAmountOut,
      keepInputBN
    };
  }

  // —— 目标配比 R*（token1:token0）——
  // amount0*(L) = (sb - sp) / (sp * sb)
  // amount1*(L) = (sp - sa)
  // R* = amount1*/amount0* = (sp - sa) * (sp * sb) / (sb - sp)
  const amount0_per_L = sb.minus(sp).div(sp.mul(sb)); // token0 侧
  const amount1_per_L = sp.minus(sa); // token1 侧
  const Rstar = amount1_per_L.mul(sp.mul(sb)).div(sb.minus(sp));

  // —— 二分搜索 y ∈ [0, amountIn] ——（根据输入边选择比例口径）
  const inDec = bnToDec(amountInBN);
  let lo = new BN(0);
  let hi = amountInBN.clone();
  let last = new BN(-1);
  const amtTol = amountToleranceBN ?? decToBn(new Decimal(1e-3));
  let iters = 0;

  // （可选）记录最佳值，用于 maxIter 触顶兜底
  let best = { y: new BN(0), out: new BN(0), err: new Decimal(Infinity), ratioStr: 'NaN' };

  const evalAt = async (y: BN) => {
    const { out, allTrade } = quote(y, inputIsMintA, slippage);
    const outDec = bnToDec(out);
    const keepDec = inDec.minus(bnToDec(y));
    // 统一口径为 token1/token0：
    // 输入A(token0)：ratio = token1_out / token0_keep
    // 输入B(token1)：ratio = token1_keep / token0_out
    const ratio = inputIsMintA ? outDec.div(keepDec) : keepDec.div(outDec);
    return { ratio, out, allTrade };
  };

  while (iters++ < maxIter) {
    const mid = lo.add(hi).divn(2);
    const { ratio, out, allTrade } = await evalAt(mid);

    // 报价未能完全成交 → 收紧上界，继续搜（常见于 tick 缓存不足）
    if (!allTrade) {
      hi = mid;
      last = mid;
      continue;
    }

    const err = ratio.minus(Rstar).abs().div(Decimal.max(Rstar, 1e-18));
    if (err.lt(best.err)) best = { y: mid, out, err, ratioStr: ratio.toString() };

    // 收敛条件：比例足够近 或 步长足够小
    if (err.lte(ratioTolerance) || mid.sub(last).abs().lte(amtTol) || hi.sub(lo).lte(amtTol)) {
      const keepBN = amountInBN.sub(mid);
      return {
        mode: 'two-tokens' as const,
        swapAmountIN: mid, // 本次 mid 的 swap 金额
        swapAmountOut: out, // 本次 mid 的对侧产出
        keepInputBN: keepBN, // 本次 mid 的剩余
        Rstar: Rstar.toString(),
        finalRatio: ratio.toString(),
        converged: err.lte(ratioTolerance),
        iters
      };
    }

    // 二分方向：基于“token1/token0”的 ratio 与 R*
    if (ratio.gt(Rstar)) {
      // token1 偏多
      if (inputIsMintA) hi = mid;
      else lo = mid;
    } else {
      // token0 偏多
      if (inputIsMintA) lo = mid;
      else hi = mid;
    }
    last = mid;
  }

  // 触达迭代上限：用 best 近似
  const keepBN = amountInBN.sub(best.y);

  return {
    mode: 'two-tokens' as const,
    swapAmountIN: best.y,
    swapAmountOut: best.out,
    keepInputBN: keepBN,
    Rstar: Rstar.toString(),
    finalRatio: best.ratioStr,
    converged: false,
    iters: maxIter,
    note: '达到最大迭代步，返回近似最优解'
  };
}
