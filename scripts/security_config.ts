import { PublicKey, SystemProgram } from '@solana/web3.js';
import fs from 'node:fs';
import path from 'node:path';
import { Program } from '@coral-xyz/anchor';
import { anchor, userWallet } from './help';

function loadIdl(): any {
  // Read the freshly built IDL from target/idl.
  // This avoids relying on Anchor.toml program id when we want to override programId explicitly.
  const p = path.join(process.cwd(), 'target', 'idl', 'lp_handler.json');
  return JSON.parse(fs.readFileSync(p, 'utf8'));
}

function getProgram(programId?: string): Program<any> {
  if (programId) {
    const idl = loadIdl();
    // Anchor Program constructor in TS expects programId to be embedded in idl.metadata.address.
    idl.metadata = idl.metadata ?? {};
    idl.metadata.address = programId;
    return new Program(idl, anchor.AnchorProvider.env());
  }
  // Default path: use Anchor workspace program (uses Anchor.toml/idl metadata).
  return anchor.workspace.lpHandler as Program<any>;
}

function getSecurityConfigPda(programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([Buffer.from('security_config')], programId)[0];
}

export const updateSecurityConfig = async (programId: string) => {
  const program = getProgram(programId);
  const securityConfig = getSecurityConfigPda(program.programId);
  const pools = process.env.SECURITY_POOLS?.split(',').map((p) => new PublicKey(p));
  if (!pools) {
    throw new Error('environment variable SECURITY_POOLS is not set');
  }
  const feeOwners = process.env.SECURITY_FEE_OWNERS?.split(',').map((p) => new PublicKey(p));
  if (!feeOwners) {
    throw new Error('environment variable SECURITY_FEE_OWNERS is not set');
  }
  const updateIx = await program.methods
    .updateSecurityConfig(
      pools.map((p) => new PublicKey(p)),
      feeOwners.map((p) => new PublicKey(p))
    )
    .accountsStrict({
      authority: userWallet.publicKey,
      securityConfig
    })
    .rpc();
  console.log('updateSecurityConfig success tx:', updateIx);
};

export const closeSecurityConfig = async (programId: string) => {
  const program = getProgram(programId);
  const securityConfig = getSecurityConfigPda(program.programId);
  const closeIx = await program.methods
    .closeSecurityConfig()
    .accountsStrict({
      authority: userWallet.publicKey,
      securityConfig,
      receiver: userWallet.publicKey
    })
    .rpc();
  console.log('closeSecurityConfig success tx:', closeIx);
};

export const initSecurityConfig = async (programId: string) => {
  const program = getProgram(programId);
  const securityConfig = getSecurityConfigPda(program.programId);
  const pools = process.env.SECURITY_POOLS?.split(',').map((p) => new PublicKey(p));
  if (!pools) {
    throw new Error('SECURITY_POOLS is not set');
  }
  const feeOwners = process.env.SECURITY_FEE_OWNERS?.split(',').map((p) => new PublicKey(p));
  if (!feeOwners) {
    throw new Error('SECURITY_FEE_OWNERS is not set');
  }
  // 说明：这里用 any 绕过 target/types 未及时更新导致的 TS 类型问题；
  // 运行前请确保你已 anchor build 生成最新 IDL/types。
  const initIx = await program.methods
    .initSecurityConfig(
      pools.map((p) => new PublicKey(p)),
      feeOwners.map((p) => new PublicKey(p))
    )
    .accountsStrict({
      authority: userWallet.publicKey,
      securityConfig,
      systemProgram: SystemProgram.programId
    })
    .rpc();
  console.log('initSecurityConfig success tx:', initIx);
};
