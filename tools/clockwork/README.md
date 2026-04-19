# Clockwork (deprecated — kept as signpost)

This folder contains no code. It's a placeholder pointing at
Clockwork, an automation / scheduling layer for Solana that let you
run "cron jobs" onchain by bundling transactions into threads the
network would execute on a schedule.

## Status

Clockwork's original maintainers ceased active development in 2023.
The code lives at https://github.com/clockwork-xyz/clockwork but
should be treated as an archived reference rather than a
production-ready dependency. Several forks exist; none of them are
the canonical continuation.

If you need onchain scheduling today, consider:

- **Client-side cron + a keeper bot.** Run an off-chain cron (e.g.
  in OpenClaw, systemd, or AWS EventBridge) that signs and submits
  your recurring transaction. Simple; no extra onchain program to
  trust.
- **Switchboard's VRF/functions.** Switchboard exposes scheduled
  function invocations, backed by their oracle network.
- **Jito's bundle submission with time-based triggers.** Less of a
  scheduler, more of a "land at this slot" primitive.
- **Clockwork forks.** Some teams maintain private forks; none are
  widely trusted.

## What Clockwork did (in short)

1. You deployed your own Solana program with whatever logic you
   wanted triggered later.
2. You created a Clockwork `Thread` account specifying:
   - A trigger (cron expression, slot number, account-change,
     immediate).
   - An instruction (or multiple) to fire when triggered.
   - A fee-payer PDA that Clockwork would use to pay for the
     execution.
3. The Clockwork worker network watched for the trigger and
   submitted the transaction on your behalf.

For a canonical example, see
[clockwork-xyz/clockwork](https://github.com/clockwork-xyz/clockwork)'s
`examples/` directory.

## Why this folder exists

The Solana Program Examples repo historically included a Clockwork
example. It was removed when the project went quiet. This README
remains as a signpost so readers searching for "Solana cron" or
"onchain scheduler" understand the current state and have
alternatives.
