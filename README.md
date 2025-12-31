```base
# 部署合约
# 注意：mainnet 需要带上 `--features mainnet`，否则会触发 Anchor 的
# DeclaredProgramIdMismatch（代码里 declare_id! 会落到 devnet 的 ProgramId）
npm run  deploy:mainnet

# 关闭合约
solana program close CkNvt3vAo9gQ288sKLwdiGL5tcLkGowmLXXjmxYvH6ZT --recipient CB5HJVasNzZ7nWJHJTvuiKm4vF9yb5YFqc9jnnPScKpB --bypass-warning

# 创建程序id
solana-keygen new --no-bip39-passphrase --silent -o /Users/hanyukai/Desktop/code/demo/solana-contract/lp_deposit/id/lp_handler-mainnet-keypair.json
solana-keygen pubkey ./id/lp_handler-mainnet-keypair.json
```
