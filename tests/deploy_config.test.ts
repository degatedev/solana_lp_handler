import { Keypair } from '@solana/web3.js';

import { buildAnchorBuildEnv } from '../scripts/deploy_config';

describe('buildAnchorBuildEnv', () => {
  it('injects SECURITY_ADMIN as the deploy wallet pubkey', () => {
    const deployer = Keypair.generate();

    const env = buildAnchorBuildEnv(deployer.publicKey.toBase58(), {
      PATH: '/usr/bin',
      NODE_ENV: 'production'
    });

    expect(env.SECURITY_ADMIN).toBe(deployer.publicKey.toBase58());
    expect(env.PATH).toBe('/usr/bin');
    expect(env.NODE_ENV).toBe('production');
  });
});
