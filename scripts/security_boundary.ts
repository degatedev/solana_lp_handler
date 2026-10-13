import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

export const DEFAULT_SECURITY_BOUNDARY_FILES = [
  'programs/lp_handler/src/lib.rs',
  'programs/lp_handler/src/security/mod.rs'
] as const;

export const DEFAULT_SECURITY_BOUNDARY_DIGEST_FILE = 'scripts/security_boundary.sha256';

type ReadFile = (filePath: string) => Buffer;

function sha256(content: Buffer | string): string {
  return createHash('sha256').update(content).digest('hex');
}

export function buildSecurityBoundaryDigest(
  filePaths: readonly string[] = DEFAULT_SECURITY_BOUNDARY_FILES,
  readFile: ReadFile = (filePath) => fs.readFileSync(filePath)
): string {
  const aggregateInput = filePaths
    .map((filePath) => `${sha256(readFile(filePath))}  ${filePath}\n`)
    .join('');
  return sha256(aggregateInput);
}

export function loadExpectedSecurityBoundaryDigest(
  digestFilePath: string = DEFAULT_SECURITY_BOUNDARY_DIGEST_FILE
): string {
  return fs.readFileSync(digestFilePath, 'utf8').trim();
}

export function verifySecurityBoundaryDigest({
  expectedDigest,
  filePaths = DEFAULT_SECURITY_BOUNDARY_FILES,
  readFile = (filePath) => fs.readFileSync(filePath)
}: {
  expectedDigest: string;
  filePaths?: readonly string[];
  readFile?: ReadFile;
}): string {
  const actualDigest = buildSecurityBoundaryDigest(filePaths, readFile);
  if (actualDigest !== expectedDigest) {
    throw new Error(
      `固定安全边界文件摘要不匹配: expected=${expectedDigest}, actual=${actualDigest}`
    );
  }
  return actualDigest;
}

export function verifySecurityBoundaryFromBaseline(
  digestFilePath: string = DEFAULT_SECURITY_BOUNDARY_DIGEST_FILE
): string {
  const normalizedPath = path.normalize(digestFilePath);
  const expectedDigest = loadExpectedSecurityBoundaryDigest(normalizedPath);
  return verifySecurityBoundaryDigest({ expectedDigest });
}
