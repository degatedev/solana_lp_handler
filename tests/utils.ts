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

/** —— 类型约束（避免“价格/比例”口径混乱）—— */
export type SqrtPriceX64 = BN;
export enum ZapSwapDirection {
  AtoB = 'AtoB',
  BtoA = 'BtoA',
  None = 'none'
}

export enum ZapMode {
  SingleTokenOnly = 'single-token-only',
  TwoTokens = 'two-tokens',
  NoSwap = 'no-swap'
}

export type ClmmQuote = {
  /** 对侧输出（raw） */
  out: BN;
  /** tick 覆盖是否足够，true 表示可完整成交 */
  allTrade: boolean;
  /** 按 slippage 计算的最小输出（raw） */
  minOut: BN;
  /** 这笔 swap 计算后的价格（sqrtPriceX64，Q64.64） */
  executionPriceX64: SqrtPriceX64;
};
export type ClmmQuoteFn = (amountIn: BN, inputIsMintA: boolean, slippage?: number) => ClmmQuote;

export type BuildClmmQuoteContext = {
  computePool: ComputeClmmPoolInfo;
  tickArrayCache: ReturnTypeFetchMultiplePoolTickArrays;
  epochInfo: EpochInfo;
  quote: ClmmQuoteFn;
};

/** 只保留“swap + 开仓”关心的字段 */
export type ZapPlan = {
  mode: ZapMode;
  swapDirection: ZapSwapDirection;
  swapAmountIN: BN;
  swapAmountOut: BN;
  swapMinOut: BN;
  amountAForPosition: BN;
  amountBForPosition: BN;
  note?: string;
};

/** —— 工具 —— */
const bnToDec = (bn: BN) => new Decimal(bn.toString());
const decToBn = (x: Decimal) => new BN(x.toFixed(0));
const sFromX64 = (x64: BN) => new Decimal(x64.toString()).div(new Decimal(Q64.toString()));

/** 构建离链报价上下文（含 tick 缓存与 epochInfo） */
export async function buildClmmQuoteContext(opts: {
  connection: Connection;
  apiPoolItem: Pick<ApiV3PoolInfoConcentratedItem, 'id' | 'programId' | 'mintA' | 'mintB' | 'config' | 'price'>;
  batchRequest?: boolean;
}): Promise<BuildClmmQuoteContext> {
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
  const quote = (amountIn: BN, inputIsMintA: boolean, slippage = 0): ClmmQuote => {
    if (amountIn.lten(0)) {
      return { out: new BN(0), allTrade: true, minOut: new BN(0), executionPriceX64: computePool.sqrtPriceX64 };
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
      out: res.amountOut.amount.raw,
      allTrade: res.allTrade,
      minOut: res.minAmountOut.amount.raw,
      // swap 后的 sqrtPriceX64（同池 swap 会改变后续开仓时的现价）
      executionPriceX64: res.executionPriceX64
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
}): Promise<ZapPlan> {
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
    let swapDirection: ZapSwapDirection = ZapSwapDirection.None;
    let swapAmountIN = new BN(0);
    let swapAmountOut = new BN(0);
    let swapMinOut = new BN(0);
    let amountAForPosition = new BN(0);
    let amountBForPosition = new BN(0);
    let note = onlyToken0 ? '区间在现价上方，仅需 token0' : '区间在现价下方，仅需 token1';
    let allTradeOk = true;

    if ((onlyToken0 && !inputIsMintA) || (!onlyToken0 && inputIsMintA)) {
      // 需要把全部换成目标边
      const quoted = quote(amountInBN, inputIsMintA, slippage ?? 0);
      swapDirection = inputIsMintA ? ZapSwapDirection.AtoB : ZapSwapDirection.BtoA;
      swapAmountIN = amountInBN;
      swapAmountOut = quoted.out;
      swapMinOut = quoted.minOut;
      allTradeOk = quoted.allTrade;
      if (!allTradeOk) note += '（警告：报价未完全成交，tick 缓存不足）';
    } else {
      // 无需 swap，直接用输入侧开仓
      if (onlyToken0) amountAForPosition = amountInBN;
      else amountBForPosition = amountInBN;
    }

    // 需要 swap 的情况下，用“最小输出”作为开仓可用量更安全（避免 swap 实际产出略低导致开仓失败）
    if (swapAmountIN.gt(new BN(0))) {
      if (onlyToken0) amountAForPosition = swapMinOut;
      else amountBForPosition = swapMinOut;
    }

    return {
      mode: ZapMode.SingleTokenOnly,
      note,
      swapDirection,
      swapAmountIN,
      swapAmountOut,
      swapMinOut,
      amountAForPosition,
      amountBForPosition
    };
  }

  // —— 二分搜索 y ∈ [0, amountIn] ——（根据输入边选择比例口径）
  const inDec = bnToDec(amountInBN);
  let lo = new BN(0);
  let hi = amountInBN.clone();
  let last = new BN(-1);
  // 注意：decToBn(1e-3) 会被 toFixed(0) 截断为 0，导致“步长收敛条件”失效。
  // 这里用最小单位 1 作为默认步长阈值（用户可通过 amountToleranceBN 自定义）。
  const amtTol = amountToleranceBN ?? new BN(1);
  let iters = 0;

  // （可选）记录最佳值，用于 maxIter 触顶兜底
  let best = { y: new BN(0), out: new BN(0), minOut: new BN(0), err: new Decimal(Infinity) };

  const evalAt = async (y: BN) => {
    const { out, minOut, allTrade, executionPriceX64 } = quote(y, inputIsMintA, slippage ?? 0);

    // 关键：你是 “swap 指令在前，openPosition 在后”，开仓时用的是 swap 后价格
    const spAfter = sFromX64(executionPriceX64);
    // swap 后价格推到区间外 → 开仓将退化为单边；这时继续增大 y 只会更偏离
    if (spAfter.lte(sa) || spAfter.gte(sb)) {
      return { ratio: new Decimal(NaN), out, minOut, allTrade: false, Rstar: new Decimal(NaN) };
    }

    // —— 目标配比 R*(token1:token0)，在 spAfter 下计算 ——（单位为 raw1/raw0）
    // amount0*(L) = (sb - sp) / (sp * sb)
    // amount1*(L) = (sp - sa)
    // R* = amount1*/amount0*
    const amount0_per_L = sb.minus(spAfter).div(spAfter.mul(sb)); // token0 侧
    const amount1_per_L = spAfter.minus(sa); // token1 侧
    const Rstar = amount1_per_L.div(Decimal.max(amount0_per_L, 1e-30));

    const outDec = bnToDec(out);
    const keepDec = inDec.minus(bnToDec(y));
    // 统一口径为 token1/token0：
    // 输入A(token0)：ratio = token1_out / token0_keep
    // 输入B(token1)：ratio = token1_keep / token0_out
    // 防止极端值导致除 0/Infinity
    const denom = inputIsMintA ? Decimal.max(keepDec, 1e-18) : Decimal.max(outDec, 1e-18);
    const numer = inputIsMintA ? outDec : keepDec;
    const ratio = numer.div(denom);
    return { ratio, out, minOut, allTrade, Rstar };
  };

  while (iters++ < maxIter) {
    const mid = lo.add(hi).divn(2);
    const { ratio, out, minOut, allTrade, Rstar } = await evalAt(mid);

    // 报价未能完全成交 → 收紧上界，继续搜（常见于 tick 缓存不足）
    if (!allTrade) {
      hi = mid;
      last = mid;
      continue;
    }

    const err = ratio.minus(Rstar).abs().div(Decimal.max(Rstar, 1e-18));
    if (err.lt(best.err)) best = { y: mid, out, minOut, err };

    // 收敛条件：比例足够近 或 步长足够小
    if (err.lte(ratioTolerance) || mid.sub(last).abs().lte(amtTol) || hi.sub(lo).lte(amtTol)) {
      const keepBN = amountInBN.sub(mid);
      return {
        mode: ZapMode.TwoTokens,
        swapDirection: inputIsMintA ? ZapSwapDirection.AtoB : ZapSwapDirection.BtoA,
        swapAmountIN: mid,
        swapAmountOut: out,
        swapMinOut: minOut,
        // 开仓用量：一边是 keep，一边用 minOut 更安全
        amountAForPosition: inputIsMintA ? keepBN : minOut,
        amountBForPosition: inputIsMintA ? minOut : keepBN
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
    mode: ZapMode.TwoTokens,
    swapAmountIN: best.y,
    swapDirection: inputIsMintA ? ZapSwapDirection.AtoB : ZapSwapDirection.BtoA,
    swapAmountOut: best.out,
    swapMinOut: best.minOut,
    amountAForPosition: inputIsMintA ? keepBN : best.minOut,
    amountBForPosition: inputIsMintA ? best.minOut : keepBN
  };
}

/**
 * 双币输入配平（可选一次 swap，然后开仓）
 *
 * 目标：在你的执行顺序是 “swap 在前，openPosition 在后” 的前提下，
 * 每次评估都使用 swap 后的 executionPriceX64 动态计算当前区间的最优配比 R*(token1/token0)，
 * 二分出需要 swap 的数量，使得 swap 后的 (amount1/amount0) ≈ R*，从而尽量两边都用满。
 */
export async function solveZapTwoSidedCLMM(opts: {
  connection: Connection;
  apiPoolItem: Pick<ApiV3PoolInfoConcentratedItem, 'id' | 'programId' | 'mintA' | 'mintB' | 'config' | 'price'>;
  tickLower: number;
  tickUpper: number;

  /** 双币输入：分别是 mintA/mintB 的最小单位数量 */
  amountAInBN: BN;
  amountBInBN: BN;

  slippage?: number;
  ratioTolerance?: number;
  amountToleranceBN?: BN;
  maxIter?: number;
}): Promise<ZapPlan> {
  const {
    connection,
    apiPoolItem,
    tickLower,
    tickUpper,
    amountAInBN,
    amountBInBN,
    slippage,
    ratioTolerance = 1e-8,
    amountToleranceBN,
    maxIter = 100
  } = opts;

  // 入参校验
  if (amountAInBN.isNeg() || amountBInBN.isNeg()) {
    throw new Error('amountAInBN/amountBInBN 不能为负数');
  }
  if (amountAInBN.isZero() && amountBInBN.isZero()) {
    throw new Error('amountAInBN 与 amountBInBN 不能同时为 0');
  }

  // 退化：单边输入直接复用单边逻辑（保持行为一致）
  if (amountAInBN.lten(0)) {
    const r = await solveZapSingleSidedCLMM({
      connection,
      apiPoolItem,
      tickLower,
      tickUpper,
      inputMint: apiPoolItem.mintB.address,
      amountInBN: amountBInBN,
      slippage,
      ratioTolerance,
      amountToleranceBN,
      maxIter
    });
    return r;
  }
  if (amountBInBN.lten(0)) {
    const r = await solveZapSingleSidedCLMM({
      connection,
      apiPoolItem,
      tickLower,
      tickUpper,
      inputMint: apiPoolItem.mintA.address,
      amountInBN: amountAInBN,
      slippage,
      ratioTolerance,
      amountToleranceBN,
      maxIter
    });
    return r;
  }

  const { computePool, quote } = await buildClmmQuoteContext({ connection, apiPoolItem });

  const sa = sFromX64(SqrtPriceMath.getSqrtPriceX64FromTick(tickLower));
  const sb = sFromX64(SqrtPriceMath.getSqrtPriceX64FromTick(tickUpper));
  const sp = sFromX64(computePool.sqrtPriceX64);

  // 区间不跨现价：开仓本身就是单边需求。双币输入在这里“配平”的正确行为通常是把另一边全部换掉。
  // （保持和单边函数一致的语义：如果只需要 token0/1，那么就把多余的一侧换成需要的一侧）
  if (sp.lte(sa) || sp.gte(sb)) {
    const onlyToken0 = sp.lte(sa); // true=仅需 token0(mintA)，false=仅需 token1(mintB)
    if (onlyToken0) {
      // 仅需 A：把 B 全换成 A
      const q = quote(amountBInBN, false, slippage ?? 0);
      return {
        mode: ZapMode.SingleTokenOnly,
        swapDirection: amountBInBN.gt(new BN(0)) ? ZapSwapDirection.BtoA : ZapSwapDirection.None,
        swapAmountIN: amountBInBN,
        swapAmountOut: q.out,
        swapMinOut: q.minOut,
        amountAForPosition: amountAInBN.add(q.minOut),
        amountBForPosition: new BN(0),
        note: '区间在现价上方，仅需 token0'
      };
    }
    // 仅需 B：把 A 全换成 B
    const q = quote(amountAInBN, true, slippage ?? 0);
    return {
      mode: ZapMode.SingleTokenOnly,
      swapDirection: amountAInBN.gt(new BN(0)) ? ZapSwapDirection.AtoB : ZapSwapDirection.None,
      swapAmountIN: amountAInBN,
      swapAmountOut: q.out,
      swapMinOut: q.minOut,
      amountAForPosition: new BN(0),
      amountBForPosition: amountBInBN.add(q.minOut),
      note: '区间在现价下方，仅需 token1'
    };
  }

  // 当前持仓比例（token1/token0），raw 口径
  const ratioNow = bnToDec(amountBInBN).div(Decimal.max(bnToDec(amountAInBN), 1e-18));

  // 决定 swap 方向：ratioNow > R* → token1 过多，swap B->A；反之 swap A->B
  // 注意：这里的 R* 不能用 swap 前 sp 固定算，因为你的 swap 会改价格；我们用动态 R* 做二分目标。
  // 先用 sp 估一个方向，不影响正确性（动态 R* 会在 evalAt 中校正），主要用来定搜索区间与单调方向。
  const amount0_per_L_sp = sb.minus(sp).div(sp.mul(sb));
  const amount1_per_L_sp = sp.minus(sa);
  const Rstar_sp = amount1_per_L_sp.div(Decimal.max(amount0_per_L_sp, 1e-30));
  const swapBtoA = ratioNow.gt(Rstar_sp);

  // 同上：默认使用 1（最小单位）避免被截断为 0
  const amtTol = amountToleranceBN ?? new BN(1);
  let lo = new BN(0);
  let hi = swapBtoA ? amountBInBN.clone() : amountAInBN.clone();
  let last = new BN(-1);
  let iters = 0;

  let best = {
    y: new BN(0),
    out: new BN(0),
    minOut: new BN(0),
    err: new Decimal(Infinity)
  };

  const evalAt = async (y: BN) => {
    // 把 y 从过多的一侧换到另一侧
    const q = swapBtoA ? quote(y, false, slippage ?? 0) : quote(y, true, slippage ?? 0);

    // 若报价未完全成交/或 swap 后把价格推到区间外，都认为该 y 不可用，交由上层收紧区间
    const spAfter = sFromX64(q.executionPriceX64);
    if (!q.allTrade || spAfter.lte(sa) || spAfter.gte(sb)) {
      return {
        ok: false,
        out: q.out,
        minOut: q.minOut,
        ratio: new Decimal(NaN),
        Rstar: new Decimal(NaN)
      };
    }

    const amount0_per_L = sb.minus(spAfter).div(spAfter.mul(sb));
    const amount1_per_L = spAfter.minus(sa);
    const Rstar = amount1_per_L.div(Decimal.max(amount0_per_L, 1e-30));

    // swap 后两边余额
    const aAfter = swapBtoA ? amountAInBN.add(q.out) : amountAInBN.sub(y);
    const bAfter = swapBtoA ? amountBInBN.sub(y) : amountBInBN.add(q.out);

    const ratio = bnToDec(bAfter).div(Decimal.max(bnToDec(aAfter), 1e-18));
    return { ok: true, out: q.out, minOut: q.minOut, ratio, Rstar, aAfter, bAfter };
  };

  // 若当前比例已经足够接近（用 swap 前 R* 粗判），直接不 swap
  const roughErr = ratioNow.minus(Rstar_sp).abs().div(Decimal.max(Rstar_sp, 1e-18));
  if (roughErr.lte(ratioTolerance)) {
    return {
      mode: ZapMode.NoSwap,
      swapDirection: ZapSwapDirection.None,
      swapAmountIN: new BN(0),
      swapAmountOut: new BN(0),
      swapMinOut: new BN(0),
      amountAForPosition: amountAInBN,
      amountBForPosition: amountBInBN
    };
  }

  while (iters++ < maxIter) {
    const mid = lo.add(hi).divn(2);
    const res = await evalAt(mid);

    if (!res.ok) {
      // 不可用：收紧上界（通常是把价格推到区间外或 tick 缓存不足）
      hi = mid;
      last = mid;
      continue;
    }

    const err = res.ratio.minus(res.Rstar).abs().div(Decimal.max(res.Rstar, 1e-18));
    if (err.lt(best.err)) {
      best = { y: mid, out: res.out, minOut: res.minOut, err };
    }

    if (err.lte(ratioTolerance) || mid.sub(last).abs().lte(amtTol) || hi.sub(lo).lte(amtTol)) {
      return {
        mode: ZapMode.TwoTokens,
        swapDirection: swapBtoA ? ZapSwapDirection.BtoA : ZapSwapDirection.AtoB,
        swapAmountIN: mid,
        swapAmountOut: res.out,
        swapMinOut: res.minOut,
        amountAForPosition: swapBtoA ? amountAInBN.add(res.minOut) : amountAInBN.sub(mid),
        amountBForPosition: swapBtoA ? amountBInBN.sub(mid) : amountBInBN.add(res.minOut)
      };
    }

    // 二分方向：比较 token1/token0
    if (res.ratio.gt(res.Rstar)) {
      // token1 偏多 → 需要更多地把 token1 换成 token0
      if (swapBtoA) lo = mid;
      else hi = mid;
    } else {
      // token0 偏多 → 需要更多地把 token0 换成 token1
      if (swapBtoA) hi = mid;
      else lo = mid;
    }
    last = mid;
  }

  // 触达迭代上限：用 best 近似
  const qBest = swapBtoA ? quote(best.y, false, slippage ?? 0) : quote(best.y, true, slippage ?? 0);
  const amountAForPosition = swapBtoA ? amountAInBN.add(qBest.minOut) : amountAInBN.sub(best.y);
  const amountBForPosition = swapBtoA ? amountBInBN.sub(best.y) : amountBInBN.add(qBest.minOut);

  return {
    mode: ZapMode.TwoTokens,
    swapDirection: swapBtoA ? ZapSwapDirection.BtoA : ZapSwapDirection.AtoB,
    swapAmountIN: best.y,
    swapAmountOut: qBest.out,
    swapMinOut: qBest.minOut,
    amountAForPosition,
    amountBForPosition,
    note: '达到最大迭代步，返回近似最优解'
  };
}
