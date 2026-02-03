import { prompt } from 'enquirer';
import { closeSecurityConfig, initSecurityConfig, updateSecurityConfig } from './security_config';
import { deploy } from './deploy';
import { closeProgram } from './closeProgram';
import { getProgramId, confirm } from './help';

async function main() {
  const response = await prompt<{ command: string }>({
    type: 'select',
    name: 'command',
    message: '请选择需要执行的命令',
    choices: [
      { name: 'DEPLOY', message: '部署合约(会自动初始化安全配置)' },
      { name: 'CLOSE_PROGRAM', message: '关闭旧合约并回收租金(会自动关闭安全配置)' },
      { name: 'INIT_SECURITY_CONFIG', message: '初始化安全配置' },
      { name: 'UPDATE_SECURITY_CONFIG', message: '更新安全配置' },
      { name: 'CLOSE_SECURITY_CONFIG', message: '关闭安全配置账户回收租金' }
    ]
  });
  switch (response.command) {
    case 'UPDATE_SECURITY_CONFIG':
      await updateSecurityConfig(await getProgramId());
      break;
    case 'INIT_SECURITY_CONFIG':
      await initSecurityConfig(await getProgramId());
      break;
    case 'CLOSE_SECURITY_CONFIG':
      await closeSecurityConfig(await confirm(getProgramId, '是否确认关闭安全配置账户回收租金(该操作不可撤销)？'));
      break;
    case 'DEPLOY':
      await deploy();
      break;
    case 'CLOSE_PROGRAM':
      await closeProgram(await confirm(getProgramId, '是否确认关闭合约(该操作不可撤销)？'));
      break;
    default:
      console.log('无效的命令');
      break;
  }
}

main();
