export function buildAnchorBuildEnv(
  securityAdmin: string,
  baseEnv: NodeJS.ProcessEnv = process.env
): NodeJS.ProcessEnv {
  return {
    ...baseEnv,
    SECURITY_ADMIN: securityAdmin
  };
}
