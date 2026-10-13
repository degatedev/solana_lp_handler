import {
  ComputeBudgetProgram,
  PublicKey,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction
} from '@solana/web3.js';
import {
  connection,
  ensureSecurityConfig,
  fee_address,
  pool_address,
  pools,
  program,
  resetSimAccount,
  securityConfig,
  submitTransaction,
  user
} from './help';

describe('pool_whitelist', () => {
  // 这三个用例会真实创建 / 更新 / 关闭 SecurityConfig PDA。
  // 先重置到未初始化状态让 init 可跑，跑完再恢复基准白名单，避免污染其他套件。
  beforeAll(async () => {
    await resetSimAccount(securityConfig);
  }, 200000);

  afterAll(async () => {
    await ensureSecurityConfig();
  }, 200000);

  it('builds init_pool_whitelist ', async () => {
    // 说明：这里用 any 绕过 target/types 未及时更新导致的 TS 类型问题；
    // 运行前请确保你已 anchor build 生成最新 IDL/types。
    const feeOwners = [fee_address];
    const initIx = await program.methods
      .initSecurityConfig(pools, feeOwners)
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
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
          initIx
        ]
      }).compileToV0Message()
    );

    const sim = await submitTransaction(tx);
    expect(sim.value.err).toBeNull();
  }, 200000);
  it('builds  update_pool_whitelist instructions', async () => {
    // 说明：这里用 any 绕过 target/types 未及时更新导致的 TS 类型问题；
    // 运行前请确保你已 anchor build 生成最新 IDL/types。

    const feeOwners = [fee_address];
    const updateIx = await program.methods
      .updateSecurityConfig(pools, feeOwners)
      .accountsStrict({
        authority: user,
        securityConfig
      })
      .instruction();

    const tx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: user,
        recentBlockhash: (await connection.getLatestBlockhash()).blockhash,
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
          updateIx
        ]
      }).compileToV0Message()
    );

    const sim = await submitTransaction(tx);
    expect(sim.value.err).toBeNull();
  }, 200000);
  it('builds closeSecurityConfig ', async () => {
    const closeIx = await program.methods
      .closeSecurityConfig()
      .accountsStrict({
        authority: user,
        securityConfig,
        receiver: user
      })
      .instruction();

    const tx = new VersionedTransaction(
      new TransactionMessage({
        payerKey: user,
        recentBlockhash: (await connection.getLatestBlockhash()).blockhash,
        instructions: [
          ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1000 }),
          closeIx
        ]
      }).compileToV0Message()
    );

    const sim = await submitTransaction(tx);
    expect(sim.value.err).toBeNull();
  }, 200000);
});
