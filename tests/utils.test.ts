import BN from 'bn.js';
import { PublicKey } from '@solana/web3.js';

import { buildCleanupSwapRemainingAccounts, ZapSwapDirection } from './utils';

describe('buildCleanupSwapRemainingAccounts', () => {
  const mintA = new PublicKey('So11111111111111111111111111111111111111112');
  const mintB = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
  const bitmap = new PublicKey('4vJ9JU1bJJE96FWSJFfVQJ8V1sC3x1D8Q3a5t7V7M7Z');
  const quotedAccount = new PublicKey('8qbHbw2BbbTHBW1sbfB7YMvLpPjN7M6x6n57Jj7mX8t');

  it('builds both cleanup directions and keeps them separate', async () => {
    const quotedAccountA = quotedAccount;
    const quotedAccountB = new PublicKey('CktRuQ2mttgXoPHN7f3MumxM4zQWwRjAq56k97X3CJid');
    const quoteRemainingAccounts = jest
      .fn()
      .mockResolvedValueOnce([quotedAccountA])
      .mockResolvedValueOnce([quotedAccountB]);

    const cleanupRemainingAccounts = await buildCleanupSwapRemainingAccounts({
      tickArrayBitmapExtension: bitmap,
      mintAAddress: mintA,
      mintBAddress: mintB,
      amountAInBN: new BN(1_000),
      amountBInBN: new BN(0),
      mainSwapDirection: ZapSwapDirection.AtoB,
      mainSwapAmountIn: new BN(400),
      mainSwapMinOut: new BN(1_000),
      amountAForPosition: new BN(580),
      amountBForPosition: new BN(980),
      quoteRemainingAccounts
    });

    expect(quoteRemainingAccounts).toHaveBeenCalledTimes(2);
    expect(quoteRemainingAccounts).toHaveBeenNthCalledWith(1, {
      amountIn: new BN(1_000),
      inputIsMintA: true
    });
    expect(quoteRemainingAccounts).toHaveBeenNthCalledWith(2, {
      amountIn: new BN(20),
      inputIsMintA: false
    });
    expect(cleanupRemainingAccounts.inputToken0.map((item) => item.pubkey)).toEqual([bitmap, quotedAccountA]);
    expect(cleanupRemainingAccounts.inputToken1.map((item) => item.pubkey)).toEqual([bitmap, quotedAccountB]);
  });

  it('falls back to one-unit quotes only when the corresponding input-side upper bound is zero', async () => {
    const quoteRemainingAccounts = jest
      .fn()
      .mockResolvedValueOnce([quotedAccount])
      .mockResolvedValueOnce([quotedAccount]);

    const cleanupRemainingAccounts = await buildCleanupSwapRemainingAccounts({
      tickArrayBitmapExtension: null,
      mintAAddress: mintA,
      mintBAddress: mintB,
      amountAInBN: new BN(0),
      amountBInBN: new BN(0),
      mainSwapDirection: ZapSwapDirection.None,
      mainSwapAmountIn: new BN(0),
      mainSwapMinOut: new BN(0),
      amountAForPosition: new BN(0),
      amountBForPosition: new BN(0),
      quoteRemainingAccounts
    });

    expect(quoteRemainingAccounts).toHaveBeenNthCalledWith(1, {
      amountIn: new BN(1),
      inputIsMintA: true
    });
    expect(quoteRemainingAccounts).toHaveBeenNthCalledWith(2, {
      amountIn: new BN(1),
      inputIsMintA: false
    });
    expect(cleanupRemainingAccounts.inputToken0).toHaveLength(1);
    expect(cleanupRemainingAccounts.inputToken1).toHaveLength(1);
  });
});
