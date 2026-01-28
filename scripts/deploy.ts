import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { checkError, execSync, spawnSyncOptions } from './help';
import { Keypair } from '@solana/web3.js';
const options = {
  stdio: 'inherit'
};
export const deploy = async () => {
  console.log('生成合约id');
  execSync(
    'solana-keygen',
    ['new', '--no-bip39-passphrase', '--silent', '-o', 'target/deploy/lp_handler-keypair.json', '--force'],
    spawnSyncOptions
  );
  const secret = Uint8Array.from(JSON.parse(fs.readFileSync('target/deploy/lp_handler-keypair.json', 'utf8')));
  const programKeypair = Keypair.fromSecretKey(secret);
  const programId = programKeypair.publicKey;
  console.log('合约ID生成成功:', programId.toBase58());
  console.log('同步合约ID...');
  execSync('anchor', ['keys', 'sync'], spawnSyncOptions);
  console.log('合约ID同步成功');
  console.log('开始构建合约...');
  execSync('anchor', ['build'], spawnSyncOptions);
  console.log('合约构建成功');
  console.log('开始部署合约...');
  execSync('anchor', ['deploy', '--provider.cluster', 'mainnet'], spawnSyncOptions);
  console.log('合约部署成功');
  console.log('开始更新安全配置...');
  // IMPORTANT: init using the freshly deployed programId (don't rely on a cached anchor.workspace program)
  const { initSecurityConfig } = await import('./security_config');

  await initSecurityConfig(programId.toBase58());
  console.log('安全配置更新成功');
};
