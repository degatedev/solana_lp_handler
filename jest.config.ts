import type { JestConfigWithTsJest } from 'ts-jest';

const config: JestConfigWithTsJest = {
  preset: 'ts-jest',
  testEnvironment: 'node',
  moduleFileExtensions: ['ts', 'js', 'tsx', 'mjs', 'cjs', 'json'],
  testMatch: ['**/*.test.ts'],
  transform: {
    '^.+\\.(ts|tsx)$': [
      'ts-jest',
      {
        tsconfig: 'tsconfig.json',
        useESM: false
      }
    ],
    '^.+\\.(js|jsx|mjs|cjs)$': ['babel-jest', { rootMode: 'root' }]
  },
  moduleNameMapper: {
    '^(\\.{1,2}/.*)\\.js$': '$1'
  },
  setupFilesAfterEnv: ['<rootDir>/tests/setup.ts'],
  globals: {
    'ts-jest': {
      tsconfig: 'tsconfig.json'
    }
  }
  // 可以直接在这里设置环境变量（会覆盖 .env 文件）
  // testEnvironmentOptions: {
  //   ANCHOR_PROVIDER_URL: 'https://api.mainnet-beta.solana.com',
  //   ANCHOR_WALLET: process.env.HOME + '/.config/solana/id.json',
  // }
};

export default config;
