# TypeScript / Next.js client — NFT with Metadata Pointer

A Next.js (`create-next-app`) front-end for the lumberjack +
Token-2022 NFT example. See the parent [README](../README.md) for
the full context.

## Running locally

```bash
yarn install
yarn dev
# open http://localhost:3000
```

By default, `utils/anchor.ts` points at the program id already
deployed to devnet. If you redeploy with your own id, update that
file.

## What the client does

- Connects a wallet (Phantom, Solflare, Backpack).
- Calls `init_player` on first use.
- Subscribes to the player's `PlayerData` PDA via
  `connection.onAccountChange` — state updates arrive via
  WebSocket instead of polling.
- Runs the same lazy-energy calculation as the program
  (`update_energy` equivalent in TypeScript) to display a live
  countdown until the next energy point.
- Calls `chop_tree` when the player clicks.
- Calls `mint_nft` to mint the player's character NFT.
- Optionally manages session keys for auto-approved gameplay.

## Regenerating types

The Anchor IDL lives in `../anchor/target/idl/extension_nft.json`
after `anchor build`. Copy the TypeScript types from
`../anchor/target/types/extension_nft.ts` into `utils/` so the
client stays in sync with the program.

## Tech

- [Next.js](https://nextjs.org/) — React app framework.
- `@coral-xyz/anchor` — typed Anchor client.
- `@solana/web3.js` — base Solana client.
- `next/font` — optimises the Inter font.

Standard Next.js pages live in `pages/`; API routes (empty in this
example) would live in `pages/api/`.
