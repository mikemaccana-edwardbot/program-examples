# Asset Leasing

Fixed-term leasing of SPL tokens with SPL collateral, time-streamed rent, and
Pyth-priced liquidation.

A lessor lists a batch of leased tokens along with the rental terms. Once a
lessee takes the lease they deposit collateral and receive the tokens. Rent
accrues per second in the collateral mint and can be swept to the lessor at
any time. If the collateral falls below the maintenance margin (priced via a
Pyth `PriceUpdateV2` account), a keeper can liquidate the position and earn a
small bounty — the rest of the collateral compensates the lessor. At expiry,
a lessee who fails to return the tokens forfeits all of their collateral.

## Instructions

| Instruction | Who calls it | What it does |
| --- | --- | --- |
| `create_lease` | Lessor | Locks the leased tokens in a program vault and publishes the terms. Lease starts in `Listed`. |
| `take_lease` | Lessee | Posts the required collateral, receives the leased tokens. Status → `Active`. |
| `pay_rent` | Anyone | Streams accrued rent (seconds × `rent_per_second`) from the collateral vault to the lessor. |
| `top_up_collateral` | Lessee | Adds more collateral to stay above the maintenance margin. |
| `return_lease` | Lessee | Returns the leased tokens, settles final rent, refunds any unused collateral, closes the lease. |
| `liquidate` | Keeper | If the position is underwater per the supplied Pyth price, seizes collateral, pays bounty to keeper + balance to lessor. |
| `close_expired` | Lessor | Cancels an unrented `Listed` lease or, after `end_ts`, claims a defaulted lessee's collateral. |

## Accounts

- `Lease` PDA — seeded by `(b"lease", lessor, lease_id)`.
- `leased_vault` PDA — seeded by `(b"leased_vault", lease)`, holds the leased tokens between listing and settlement.
- `collateral_vault` PDA — seeded by `(b"collateral_vault", lease)`, escrows the lessee's collateral.

## Pyth integration notes

The `liquidate` instruction reads a `PriceUpdateV2` account owned by the
canonical Pyth Solana Receiver program
(`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`). The price must quote one
leased token in collateral-token units. The program decodes the relevant
fields manually — it does **not** pull in `pyth-solana-receiver-sdk` because
that crate currently has a transitive `borsh` conflict with `anchor-lang`
1.0.0 (see `program-examples/.github/.ghaignore` — `oracles/pyth/anchor` is
flagged for the same reason).

Staleness is enforced (`publish_time` must be within the last 60 seconds and
must not be in the future).

## Running the tests

LiteSVM-based Rust tests live under `programs/asset-leasing/tests/` and load
the built program via `include_bytes!`, so the `.so` must exist first.

```bash
anchor build
cargo test
```

The tests cover the full lifecycle, including a mocked Pyth price drop that
triggers liquidation and a healthy-position check that must refuse to
liquidate.
