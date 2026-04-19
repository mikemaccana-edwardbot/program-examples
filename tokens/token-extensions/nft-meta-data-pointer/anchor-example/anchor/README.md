# Anchor program — NFT with Metadata Pointer

The Anchor program for the lumberjack + Token-2022 NFT example.
See the parent [README](../README.md) for a full walkthrough of the
extension, the energy system, session keys and the game logic.

## Build, deploy, test

```bash
anchor build
anchor deploy
# copy the program id from the build output into:
#   Anchor.toml
#   programs/extension_nft/src/lib.rs (declare_id!)
#   ../app/utils/anchor.ts
#   unity project's AnchorService
anchor build
anchor deploy
anchor test
```

## Program layout

```
programs/extension_nft/src/
├── lib.rs               — #[program] wiring
├── constants.rs         — MAX_ENERGY, TIME_TO_REFILL_ENERGY, etc.
├── errors.rs            — GameErrorCode (NotEnoughEnergy, WrongAuthority)
├── instructions/
│   ├── init_player.rs   — creates PlayerData + GameData PDAs
│   ├── chop_tree.rs     — lazy-updates energy, spends 1, gains 1 wood
│   └── mint_nft.rs      — creates Token-2022 NFT with MetadataPointer +
│                          embedded TokenMetadata + additional_metadata
└── state/
    ├── player_data.rs   — { authority, energy, last_login, wood }
    └── game_data.rs     — global counters
```
