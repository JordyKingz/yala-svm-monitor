# Solana Filtered Monitor

A high-performance CLI for targeted, filter-driven monitoring of Solana transactions with a focus on the YU token ecosystem. Use JSON-defined monitors, intelligent slot filtering, and multi-channel notifications to stay on top of liquidity, bridges, and supply changes.

<div align="center">
  <img src="public/solanaLogoMark.png" alt="Solana Logo" width="80" height="80" style="vertical-align: middle;">
</div>

![Solana Indexer](https://img.shields.io/badge/Solana-Indexer-blue?style=for-the-badge&logo=solana)
![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust)

## Why Filtered Monitoring?

- Config-driven detection pipeline loading monitors from `config/monitors/*.json` and alert templates from `config/alerts/*.json`
- Live and historical slot processing with automatic checkpoint resume (`slot_checkpoint.json`)
- Intelligent slot selection via YU-focused, selective, and pre-filter modes to reduce RPC load
- Concurrent transaction extraction with automatic RPC failover and retry handling
- Multi-channel alerting (Telegram, Slack, Discord, database) plus on-disk storage collections for analytics

## Quick Start

### Prerequisites
- Rust 1.70+
- SQLite3 (for local persistence used by other CLI components)
- Solana RPC endpoint (set `SOLANA_RPC_URL` or pass `--rpc-url`)
- Optional: Telegram bot token/chat ID and Slack webhook for notifications

### Setup

```bash
git clone https://github.com/yourusername/solana-indexer.git
cd solana-indexer   # adjust if your workspace differs
cp env.example .env           # populate SOLANA_RPC_URL, Telegram/Slack secrets, etc.
cargo build --release
```

Update `.env` (or your shell) with at least:

```bash
export SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
export TELEGRAM_BOT_TOKEN=...
export TELEGRAM_CHAT_ID=...
export SLACK_WEBHOOK_URL=...
```

### Run the filtered monitor

```bash
# Stream live slots with config-driven filters (recommended)
cargo run --bin monitor_with_filters

# Replay specific slots (comma-separated list or JSON array)
cargo run --bin monitor_with_filters -- monitor 251432100,251432101

# Replay using JSON slot list file
cargo run --bin monitor_with_filters -- monitor '[251432100, 251432101]'

# Override RPC URL for a single run
cargo run --bin monitor_with_filters -- --rpc-url https://solana-mainnet.g.alchemy.com/v2/<KEY>
```

Useful subcommands:

```bash
# Generate a standalone filter config file
cargo run --bin monitor_with_filters -- generate-config filters.json

# Display Telegram credential setup checklist
cargo run --bin monitor_with_filters -- telegram-setup

# Test the filters against a single slot
cargo run --bin monitor_with_filters -- test 251432100
```

The monitor will resume from `slot_checkpoint.json` if present and report a storage summary for any collections populated by filter actions.

## Configuration Layout

```
config/
  monitors/       # JSON monitor definitions (one file per category)
  alerts/         # Channel templates referenced by monitor actions
  optimization.json
  optimization_yu_focused.json
```

- `config/monitors/*.json` — core detection rules (see catalog below).
- `config/alerts/*.json` — channel templates keyed by alert ID (Telegram, Slack, Discord).
- `config/optimization.json` — generic pre-filter settings (program/token allowlist, concurrency).
- `config/optimization_yu_focused.json` — YU-only mode that skips slots with no YU activity.
- `slot_checkpoint.json` — automatically maintained progress marker for live streaming.
- `HACK_SLOT` / `START_SLOT` env vars — optional overrides for starting slot or quick experiments.

To bootstrap a config directory from scratch:

```bash
mkdir -p config/monitors config/alerts
cargo run --bin monitor_with_filters -- generate-config config/monitors/example.json
```

## Filtering Pipeline Highlights

- **FilteredTransactionMonitor** — extracts transactions per slot, evaluates filters, and dispatches actions.
- **ConcurrentSlotProcessor** — batches work across multiple slots (controlled via `MAX_CONCURRENT_SLOTS`).
- **SelectiveMonitor** — dynamically skips quiet slots by sampling account/program activity.
- **YuFocusedFilter** — when `config/optimization_yu_focused.json` exists, only processes slots containing YU token balances.
- **SlotPreFilter** — fallback allowlist using `config/optimization.json` to avoid scanning irrelevant programs.
- **NotificationManager** — retains "database" alerts for dashboards and internal health checks.
- **ConfigManager** — hot-loads JSON monitors + alerts so updates do not require recompilation.

## Monitor Catalog (config/monitors)

### Bridges (`bridge.json`)
- **Large YU LayerZero Bridge [OLD]** (`yu_layerzero_large_bridge`)
  - Conditions: `ProgramInvoked` `6doghB248px58JSSwG4qejQ46kFMW4AMj7vzJnWZHNZn` and `TokenTransfer` YU ≥ 1,000,000
  - Actions: store in `large_layerzero_bridges`
  - Alerts: `yu_layerzero_bridge_telegram`, `yu_layerzero_bridge_discord`, `yu_layerzero_bridge_slack`
- **Large YU LayerZero Bridge [NEW]** (`yu_layerzero_large_bridge_new`)
  - Conditions: `ProgramInvoked` `3fCoNdCEoEcERakCPM17NjLE9AocA86LMwRRWDpzjLVh` and `TokenTransfer` YU ≥ 1,000,000
  - Actions: store in `large_layerzero_bridges`
  - Alerts: `yu_layerzero_bridge_telegram`, `yu_layerzero_bridge_discord`, `yu_layerzero_bridge_slack`

### Burns (`burns.json`)
- **YU Token Burn ≥ 10M** (`yuya_burn_10m`)
  - Conditions: `TokenBurn` YU ≥ 10,000,000
  - Actions: store in `large_burns`
  - Alerts: `yuya_burn_telegram`, `yuya_burn_discord`, `yuya_burn_slack`
- **YU Token Burn ≥ 1M** (`yuya_burn_1m`)
  - Conditions: `TokenBurn` YU ≥ 1,000,000
  - Actions: alert (`High` severity → `database`) and store in `medium_burns`
  - Alerts: `yuya_burn_telegram`, `yuya_burn_discord`, `yuya_burn_slack`

### Mints (`mints.json`)
- **YU Token Mint ≥ 30M** (`yuya_mint_30m`)
  - Conditions: `TokenMint` YU ≥ 30,000,000
  - Actions: store in `critical_mints`
  - Alerts: `yuya_mint_telegram`, `yuya_large_mint_discord`, `yuya_mint_slack`
- **YU Token Mint ≥ 10M** (`yuya_mint_10m`)
  - Conditions: `TokenMint` YU ≥ 10,000,000
  - Actions: store in `large_mints`
  - Alerts: `yuya_mint_telegram`, `yuya_large_mint_discord`, `yuya_mint_slack`
- **YU Token Mint ≥ 1M** (`yuya_mint_1m`)
  - Conditions: `TokenMint` YU ≥ 1,000,000
  - Actions: alert (`Medium` severity → `database`) and store in `medium_mints`
  - Alerts: `yuya_mint_telegram`, `yuya_large_mint_discord`, `yuya_mint_slack`

### Swaps (`swaps.json`)
- **All YU Raydium Swaps** (`yu_raydium_all_swaps`)
  - Conditions: `TokenTransfer` YU ≥ 1,000,000 plus Raydium program (`675kPX9…`, `routeUGW…`, or `CAMMC…`)
  - Actions: store in `raydium_yu_swaps`
  - Alerts: `yuya_raydium_swap_telegram`, `yuya_raydium_swap_discord`, `yuya_raydium_swap_slack`
- **Large YU Jupiter V6 Swaps** (`yu_jupiter_v6_large_swap`)
  - Conditions: `ProgramInvoked` `JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4` and `TokenTransfer` YU ≥ 1,000,000
  - Actions: store in `large_jupiter_yu_swaps`
  - Alerts: `yuya_jupiter_swap_telegram`, `yuya_jupiter_swap_discord`, `yuya_jup_swap_slack`
- **YU–USDC Pair Swaps** (`yu_usdc_pair_swap`)
  - Conditions: `TokenTransfer` YU ≥ 500,000 and `TokenTransfer` USDC ≥ 500,000 involving Raydium/Jupiter programs
  - Actions: store in `yu_usdc_pair_swaps`
  - Alerts: `yu_usdc_pair_telegram`, `yu_usdc_pair_discord`, `yuya_swap_slack`

## Alert Catalog (config/alerts)

### Telegram (`telegram_notifications.json`)
- `yuya_mint_telegram` — Large YU mint detected
- `yuya_burn_telegram` — YU burn detected
- `yuya_swap_telegram` — Generic large swap alert
- `yuya_raydium_swap_telegram` — Raydium-specific swap
- `yuya_jupiter_swap_telegram` — Jupiter aggregator swap
- `yuya_orca_swap_telegram` — Orca Whirlpool swap
- `yu_usdc_pair_telegram` — YU–USDC pair swap
- `yu_layerzero_bridge_telegram` — LayerZero bridge transfer
- `yu_wormhole_bridge_telegram` — Wormhole bridge activity

### Slack (`slack_notifications.json`)
- `yuya_mint_slack` — Large YU mint
- `yuya_burn_slack` — YU burn
- `yuya_raydium_swap_slack` — Raydium swap
- `yuya_jup_swap_slack` — Jupiter swap
- `yuya_swap_slack` — Channel-agnostic swap template
- `yu_layerzero_bridge_slack` — LayerZero bridge

### Discord (`discord_notifications.json`)
- `yuya_large_mint_discord` — Large YU mint
- `yuya_burn_discord` — YU burn
- `yuya_swap_discord` — Generic swap
- `yuya_raydium_swap_discord` — Raydium swap
- `yuya_jupiter_swap_discord` — Jupiter swap
- `yuya_orca_swap_discord` — Orca swap
- `yu_usdc_pair_discord` — YU–USDC pair swap
- `yu_layerzero_bridge_discord` — LayerZero bridge
- `yu_wormhole_bridge_discord` — Wormhole bridge

> ℹ️ Monitors reference alert IDs; keep alert names consistent when adding new monitors so templates resolve correctly.

## Notification Setup

1. **Telegram**
   - Create a bot via @BotFather, obtain token and target chat ID.
   - Set `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`.
   - Run `cargo run --bin monitor_with_filters -- telegram-setup` for a checklist.

2. **Slack**
   - Create an incoming webhook.
   - Set `SLACK_WEBHOOK_URL` in your environment or `.env`.

3. **Discord**
   - Replace the placeholder webhook URLs in `config/alerts/discord_notifications.json` with your server webhooks.

4. **Database channel**
   - Alerts with channel `database` are stored locally via `NotificationManager`; surface them in dashboards or the TUI logger.

## Performance & Optimization

- `MAX_CONCURRENT_SLOTS` (env) — controls concurrency (default 20).
- `config/optimization.json` — loads `SlotPreFilter` for allowlisted addresses and tokens.
- `config/optimization_yu_focused.json` — enables `YuFocusedFilter` to skip non-YU slots (saves ~99% of RPC calls during backfills).
- Automatic back-pressure: monitor switches between batch catch-up (up to 500 slots) and real-time streaming, persisting checkpoints after each batch.
- RPC failover handled by `RpcClientWithFailover` with exponential backoff.

## Observability

Launch the Prometheus + Grafana stack for dashboards:

```bash
docker-compose --profile monitoring up -d
```

Dashboards surface slot rate, matches per minute, and monitor health (see `docs/MONITORING_SETUP.md` for credentials and customization). Storage collections can be exported to Grafana via SQLite or forwarded to external analytics.

## Additional CLI Tools

The repository also ships complementary commands:

```bash
cargo run -- track slots --leaders        # classic slot tracker
cargo run -- track wallets list --detailed
cargo run -- logger                       # structured log viewer
cargo run -- performance-benchmark --duration 60
```

These tools share configuration with the filtered monitor but are secondary to the JSON-defined monitoring pipeline.

## Development

```bash
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

Contributions are welcome; open an issue or PR with new filters, alert templates, or performance improvements.

## License

MIT License — see [LICENSE](LICENSE) for details.
