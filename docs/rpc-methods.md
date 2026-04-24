# Solana RPC methods used in program-examples

Scanned: /opt/clawsimple/data/program-examples + /opt/clawsimple/data/program-examples-litesvm
Scan date: 2026-04-24T19:29:23Z
Files scanned: 101
Matches found: 250 (after allowlist filter)
Non-allowlisted matches skipped: 785

## Top methods (by total uses)

```
method                             count  files  examples  bar
sendTransaction                    113    61     33        |████████████████████████████████████████|
getLatestBlockhash                 45     17     11        |████████████████                        |
getBalance                         35     16     6         |████████████                            |
getMultipleAccounts                24     12     2         |████████                                |
requestAirdrop                     14     12     8         |█████                                   |
getMinimumBalanceForRentExemption  6      4      4         |██                                      |
getTokenAccountBalance             4      1      1         |█                                       |
getAccountInfo                     2      2      2         |█                                       |
getSignaturesForAddress            2      2      2         |█                                       |
getTransaction                     2      2      2         |█                                       |
getVersion                         2      2      2         |█                                       |
getSlot                            1      1      1         |█                                       |
```

## CSV

```
method,count,files,examples,langs
sendTransaction,113,61,33,rust|ts
getLatestBlockhash,45,17,11,rust|ts
getBalance,35,16,6,rust|ts
getMultipleAccounts,24,12,2,rust
requestAirdrop,14,12,8,ts
getMinimumBalanceForRentExemption,6,4,4,ts
getTokenAccountBalance,4,1,1,ts
getAccountInfo,2,2,2,ts
getSignaturesForAddress,2,2,2,ts
getTransaction,2,2,2,ts
getVersion,2,2,2,ts
getSlot,1,1,1,ts
```

## Reproduce

Run `docs/rpc-methods-scan.sh` from anywhere; paths are absolute. Requires `ripgrep` and `python3`.
