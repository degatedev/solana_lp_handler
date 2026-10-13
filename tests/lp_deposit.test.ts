import { swapAndDeposit } from './fixtures';

describe('lp_deposit', () => {
  it('lp_deposit test', async () => {
    const { transactionResult } = await swapAndDeposit();

    expect(transactionResult.value.err).toBeNull();
  }, 500000);

  it('rejects mismatched quoted mode', async () => {
    const { quotedMode } = await swapAndDeposit();
    const mismatchedQuotedMode = quotedMode === 0 ? 1 : 0;
    const { transactionResult } = await swapAndDeposit({
      quotedMode: mismatchedQuotedMode
    });

    expect(transactionResult.value.err).not.toBeNull();
    expect((transactionResult.value.logs || []).join('\n')).toContain('Execution mode no longer matches quoted mode');
  }, 500000);
});
