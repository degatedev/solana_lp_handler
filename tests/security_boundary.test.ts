import { createHash } from 'node:crypto';

import {
  DEFAULT_SECURITY_BOUNDARY_FILES,
  buildSecurityBoundaryDigest,
  verifySecurityBoundaryDigest
} from '../scripts/security_boundary';

describe('security boundary digest', () => {
  it('builds a deterministic aggregate sha256 from fixed files', () => {
    const files = ['programs/lp_handler/src/lib.rs', 'programs/lp_handler/src/security/mod.rs'];
    const contents = new Map<string, string>([
      [files[0], 'lib-contents'],
      [files[1], 'security-contents']
    ]);

    const digest = buildSecurityBoundaryDigest(files, (file) => {
      const content = contents.get(file);
      if (!content) {
        throw new Error(`unexpected file ${file}`);
      }
      return Buffer.from(content, 'utf8');
    });

    const first = createHash('sha256').update('lib-contents').digest('hex');
    const second = createHash('sha256').update('security-contents').digest('hex');
    const aggregateInput = `${first}  ${files[0]}\n${second}  ${files[1]}\n`;
    const expected = createHash('sha256').update(aggregateInput).digest('hex');

    expect(digest).toBe(expected);
  });

  it('uses the fixed security boundary file list', () => {
    expect(DEFAULT_SECURITY_BOUNDARY_FILES).toEqual([
      'programs/lp_handler/src/lib.rs',
      'programs/lp_handler/src/security/mod.rs'
    ]);
  });

  it('throws when the current digest does not match the baseline', () => {
    expect(() =>
      verifySecurityBoundaryDigest({
        expectedDigest: 'baseline',
        filePaths: ['programs/lp_handler/src/lib.rs'],
        readFile: () => Buffer.from('changed', 'utf8')
      })
    ).toThrow('固定安全边界文件摘要不匹配');
  });
});
