```base
# 部署合约
# 本分支统一使用主网（生产）配置
npm run  deploy

# 关闭合约
solana program close Fo5wRqZVGgJt1qmShvF84yFxRo1jxVwumqJMPUnqpbKH --recipient CB5HJVasNzZ7nWJHJTvuiKm4vF9yb5YFqc9jnnPScKpB --bypass-warning

# 创建程序id
solana-keygen new --no-bip39-passphrase --silent -o ./id/lp_handler-keypair.json
solana-keygen pubkey ./id/lp_handler-keypair.json
```
