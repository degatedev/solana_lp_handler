require('dotenv').config({ path: '.env' });
import { PublicKey } from '@solana/web3.js';
import os from 'os';
import path from 'path';
import * as anchor from '@coral-xyz/anchor';
import { Program } from '@coral-xyz/anchor';
import { LpHandler } from '../target/types/lp_handler';
import { spawnSync, SpawnSyncOptions, SpawnSyncReturns } from 'child_process';
import { prompt } from 'enquirer';

// AnchorProvider.env() 读取的钱包路径不支持 `~` 展开，这里手动处理一下。
const walletPath = process.env.ANCHOR_WALLET;
if (walletPath && walletPath.startsWith('~/')) {
  process.env.ANCHOR_WALLET = path.join(os.homedir(), walletPath.slice(2));
} else if (walletPath === '~') {
  process.env.ANCHOR_WALLET = os.homedir();
}

anchor.setProvider(anchor.AnchorProvider.env());

export const program = anchor.workspace.lpHandler as Program<LpHandler>;

export const securityConfig = PublicKey.findProgramAddressSync([Buffer.from('security_config')], program.programId)[0];

export const userWallet = anchor.AnchorProvider.env().wallet;

export { anchor };

export const checkError = (result: SpawnSyncReturns<any>) => {
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`command failed, exit code=${result.status}`);
  }
};

export const spawnSyncOptions: SpawnSyncOptions = {
  stdio: 'inherit'
};

export const execSync = (command: string, args: string[], options: SpawnSyncOptions = spawnSyncOptions) => {
  console.log(`执行命令: ${command} ${args.join(' ')}`);
  const result = spawnSync(command, args, options);
  checkError(result);
  return result;
};

export const getProgramId = async () => {
  const response = await prompt<{ programId: string }>({
    type: 'input',
    name: 'programId',
    message: '请输入合约programId'
  });
  const programId = response.programId;
  if (!programId) {
    throw new Error('programId is required');
  }
  const accountInfo = await anchor.getProvider().connection.getAccountInfo(new PublicKey(programId));
  if (!accountInfo) {
    throw new Error('合约不存在');
  }
  return programId;
};

export const confirm = async <T>(fn: () => Promise<T>, message: string = '是否确认执行该操作(该操作不可撤销)？') => {
  const response = await prompt<{ confirm: boolean }>({
    type: 'confirm',
    name: 'confirm',
    message
  });
  if (!response.confirm) {
    console.log('操作已终止');
    process.exit(0);
  }
  return await fn();
};
