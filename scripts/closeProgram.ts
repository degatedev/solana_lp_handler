import { execSync, userWallet } from './help';
import { closeSecurityConfig } from './security_config';

export const closeProgram = async (programId: string) => {
  console.log('开始关闭security_config pda 账户回收租金');
  await closeSecurityConfig(programId);
  console.log('security_config pda 账户租金回收成功');
  console.log('开始关闭合约...');
  execSync('solana', [
    'program',
    'close',
    programId, // 用用户输入的 programId
    '--recipient',
    userWallet.publicKey.toBase58(),
    '--bypass-warning'
  ]);
  console.log('合约关闭成功');
  // 执行命令
};
