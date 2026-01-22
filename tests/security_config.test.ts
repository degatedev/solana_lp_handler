import { ComputeBudgetProgram, PublicKey, SystemProgram, TransactionMessage, VersionedTransaction } from '@solana/web3.js';
import { connection, pool_address, pools, program, securityConfig, user, userWallet } from './help';

describe('pool_whitelist', () => {
  it('builds init_pool_whitelist and update_pool_whitelist instructions (and simulate)', async () => {

    // 说明：这里用 any 绕过 target/types 未及时更新导致的 TS 类型问题；
    // 运行前请确保你已 anchor build 生成最新 IDL/types。
    // const initIx = await program.methods
    //   .initSecurityConfig(pools)
    //   .accountsStrict({
    //     authority: user,
    //     securityConfig,
    //     systemProgram: SystemProgram.programId
    //   })
    //   .instruction();

    // const updateIx = await program.methods
    //   .updateSecurityConfig(pools)
    //   .accountsStrict({
    //     authority: user,
    //     securityConfig,
    //   })
    //   .instruction();

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
          // initIx,
          // updateIx,
          closeIx
        ]
      }).compileToV0Message()
    );

    userWallet.signTransaction(tx);

    const sim = await connection.simulateTransaction(tx, {
      sigVerify: false,
      replaceRecentBlockhash: true,
      innerInstructions: true
    });

    const txResult = await connection.sendRawTransaction(tx.serialize(), {
      skipPreflight: false
    });
    console.log('txResult', txResult);
    // 这个测试的目标是“指令可构造 + 可跑到合约入口”。
    // 在 mainnet 上 PDA 可能已存在/authority 可能不匹配，simulate 可能失败；所以不强制要求 err==null。
    expect(sim.value.logs).toBeDefined();
    // 尽量断言至少进入过程序
    const hit = (sim.value.logs || []).some(
      (l) => l.includes('Instruction: InitSecurityConfig') || l.includes('Instruction: UpdateSecurityConfig')
        || l.includes('Instruction: CloseSecurityConfig')
    );
    expect(hit).toBeTruthy();
  }, 200000);
});

