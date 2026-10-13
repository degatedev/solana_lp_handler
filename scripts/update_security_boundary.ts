import fs from 'node:fs';
import {
  buildSecurityBoundaryDigest,
  DEFAULT_SECURITY_BOUNDARY_DIGEST_FILE,
  DEFAULT_SECURITY_BOUNDARY_FILES
} from './security_boundary';

const digest = buildSecurityBoundaryDigest(DEFAULT_SECURITY_BOUNDARY_FILES);
fs.writeFileSync(DEFAULT_SECURITY_BOUNDARY_DIGEST_FILE, digest + '\n', 'utf8');
console.log(`security_boundary.sha256 updated: ${digest}`);
