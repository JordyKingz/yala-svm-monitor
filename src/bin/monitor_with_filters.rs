use anyhow::{Result, Context};
use clap::{Parser, Subcommand};
use index_cli::{
    filtered_monitor::{FilteredTransactionMonitor, save_filter_config, create_example_filter_config},
    telegram_notifier::print_telegram_setup_instructions,
    rpc_client_with_failover::RpcClientWithFailover,
    concurrent_slot_processor::ConcurrentSlotProcessor,
    slot_pre_filter::SlotPreFilter,
    selective_monitor::SelectiveMonitor,
    yu_focused_filter::YuFocusedFilter,
};
use tracing::error;
use colored::*;
use std::env;
use std::time::Duration;
use tokio::time::sleep;
use std::path::Path;
use std::fs;
use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Option<Commands>,

    /// Custom filter configuration file (JSON) - deprecated, use config directory instead
    #[clap(short, long)]
    filter_config: Option<String>,

    /// RPC URL for Solana connection
    #[clap(short, long, env = "SOLANA_RPC_URL")]
    rpc_url: Option<String>,

    /// Slots to monitor (when no subcommand is provided)
    slots: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Monitor slots with filters (default command)
    Monitor {
        /// Slots to monitor (comma-separated or JSON array)
        /// If not provided, will use HACK_SLOT from environment
        /// If HACK_SLOT not set, will monitor live slots
        slots: Option<String>,
    },

    /// Generate example filter configuration
    GenerateConfig {
        /// Output file path
        #[clap(default_value = "filters.json")]
        output: String,
    },

    /// Show Telegram setup instructions
    TelegramSetup,

    /// Test filters with a specific slot
    Test {
        /// Slot to test
        slot: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into())
        )
        .init();

    // Load environment variables
    dotenv::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Monitor { slots }) => {
            monitor_slots(slots, cli.filter_config, cli.rpc_url).await?;
        },

        Some(Commands::GenerateConfig { output }) => {
            generate_config(&output)?;
        },

        Some(Commands::TelegramSetup) => {
            print_telegram_setup_instructions();
        },

        Some(Commands::Test { slot }) => {
            test_slot(slot, cli.filter_config, cli.rpc_url).await?;
        },

        None => {
            // Default to monitor command with provided slots or live monitoring
            monitor_slots(cli.slots, cli.filter_config, cli.rpc_url).await?;
        },
    }

    Ok(())
}

async fn monitor_slots(
    slots_opt: Option<String>,
    filter_config: Option<String>,
    rpc_url: Option<String>,
) -> Result<()> {
    println!("{}", "🔍 Solana Transaction Monitor with Filters".bright_cyan().bold());
    println!("{}", "==========================================".bright_cyan());

    let rpc_url = rpc_url.unwrap_or_else(|| {
        env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string())
    });

    // Check if config directory exists
    let config_dir = std::path::Path::new("config");
    let use_config_dir = config_dir.exists() && config_dir.is_dir();

    // Check for slots from command line or HACK_SLOT env
    let slots_to_monitor = match slots_opt {
        Some(s) => Some(s),
        None => env::var("HACK_SLOT").ok().filter(|s| !s.trim().is_empty()),
    };

    match slots_to_monitor {
        Some(slots_str) => {
            // Monitor specific slots
            monitor_specific_slots(slots_str, filter_config, rpc_url, use_config_dir).await
        },
        None => {
            // Monitor live slots
            println!("📡 Starting live slot monitoring...");
            monitor_live_slots(filter_config, rpc_url, use_config_dir).await
        }
    }
}

async fn monitor_specific_slots(
    slots_str: String,
    filter_config: Option<String>,
    rpc_url: String,
    use_config_dir: bool,
) -> Result<()> {
    // Parse slots
    let slots: Vec<u64> = if slots_str.starts_with('[') {
        serde_json::from_str(&slots_str).context("Failed to parse slots JSON")?
    } else {
        slots_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    };

    if slots.is_empty() {
        return Err(anyhow::anyhow!("No valid slots provided"));
    }

    println!("📊 Monitoring {} slots", slots.len());
    println!("🌐 RPC: {}", rpc_url.bright_blue());

    // Show filter config status
    if use_config_dir {
        println!("📁 Using config directory: {}", "config".bright_yellow());
    } else if let Some(ref config_path) = filter_config {
        println!("📋 Using filter config: {}", config_path.bright_yellow());
    } else {
        println!("📋 Using default YUYA mint filters");
    }

    // Check Telegram status
    let telegram_enabled = env::var("TELEGRAM_BOT_TOKEN").is_ok() &&
        env::var("TELEGRAM_CHAT_ID").is_ok();

    if telegram_enabled {
        println!("📱 Telegram notifications: {}", "Enabled".bright_green());
    } else {
        println!("📱 Telegram notifications: {} (set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID to enable)",
                 "Disabled".bright_red());
    }

    println!();

    // Create monitor
    let monitor = if use_config_dir {
        FilteredTransactionMonitor::from_config_dir(rpc_url, "config").await?
    } else {
        FilteredTransactionMonitor::new(rpc_url, filter_config).await?
    };

    let mut total_matched = 0;
    let mut total_scanned = 0;

    // Process each slot
    for slot in slots {
        println!("⚙️  Processing slot {}...", slot);

        match monitor.monitor_slot(slot).await {
            Ok(matched_transactions) => {
                let matched_count = matched_transactions.len();
                println!("  ✅ Found {} matching transactions", matched_count.to_string().bright_green());

                total_matched += matched_count;
                total_scanned += 1;

                // Show matched transactions
                for tx in &matched_transactions {
                    println!("    📌 {} - Filters: {}",
                             &tx.transaction.signature[..20],
                             tx.matched_filters.join(", ").bright_yellow()
                    );
                }
            },
            Err(e) => {
                println!("  ❌ Error: {}", e.to_string().bright_red());
                error!("Failed to monitor slot {}: {}", slot, e);
            }
        }
    }

    println!("\n{}", "📈 Monitoring Summary".bright_magenta().bold());
    println!("{}", "====================".bright_magenta());
    println!("Slots processed: {}", total_scanned);
    println!("Total matches: {}", total_matched.to_string().bright_green());

    // Show storage summary
    let storage_summary = monitor.get_storage_summary().await;
    if !storage_summary.is_empty() {
        println!("\n💾 Storage Collections:");
        for (collection, count) in storage_summary {
            println!("  • {}: {} transactions", collection.bright_cyan(), count);
        }
    }

    Ok(())
}

async fn monitor_live_slots(
    filter_config: Option<String>,
    rpc_url: String,
    use_config_dir: bool,
) -> Result<()> {
    const CHECKPOINT_FILE: &str = "slot_checkpoint.json";

    println!("🌐 RPC: {}", rpc_url.bright_blue());

    // Show filter config status
    if use_config_dir {
        println!("📁 Using config directory: {}", "config".bright_yellow());
    } else if let Some(ref config_path) = filter_config {
        println!("📋 Using filter config: {}", config_path.bright_yellow());
    } else {
        println!("📋 Using default YUYA mint filters");
    }

    // Check Telegram status
    let telegram_enabled = env::var("TELEGRAM_BOT_TOKEN").is_ok() &&
        env::var("TELEGRAM_CHAT_ID").is_ok();

    if telegram_enabled {
        println!("📱 Telegram notifications: {}", "Enabled".bright_green());
    } else {
        println!("📱 Telegram notifications: {} (set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID to enable)",
                 "Disabled".bright_red());
    }

    println!();

    // Create RPC client with failover to get current slot
    let rpc_client = Arc::new(RpcClientWithFailover::new(rpc_url.clone()));

    // Create monitor
    let monitor = if use_config_dir {
        FilteredTransactionMonitor::from_config_dir(rpc_url.clone(), "config").await?
    } else {
        FilteredTransactionMonitor::new(rpc_url.clone(), filter_config).await?
    };

    let mut total_matched = 0;
    let mut total_scanned = 0;
    let mut consecutive_errors = 0;

    // Check for existing checkpoint
    let checkpoint = SlotCheckpoint::load(CHECKPOINT_FILE)?;
    let start_slot = if let Some(ref cp) = checkpoint {
        println!("📂 Found checkpoint from slot {} (processed {} slots, {} matches)",
                 cp.last_processed_slot,
                 cp.total_slots_processed,
                 cp.total_matches_found
        );
        cp.last_processed_slot + 1
    } else if let Ok(start_slot_str) = env::var("START_SLOT") {
        let slot = start_slot_str.trim().parse::<u64>()
            .context("Invalid START_SLOT value")?;
        println!("🎯 Starting from configured slot: {}", slot);
        slot
    } else {
        let current = rpc_client.get_slot().await?;
        println!("🚀 Starting from current slot: {}", current);
        current
    };

    // Initialize counters from checkpoint if available
    if let Some(cp) = checkpoint {
        total_matched = cp.total_matches_found;
        total_scanned = cp.total_slots_processed;
    }

    println!("Press Ctrl+C to stop\n");

    let mut current_slot = start_slot;
    let monitor_arc = Arc::new(monitor);

    // Get max concurrent slots from env
    let max_concurrent = env::var("MAX_CONCURRENT_SLOTS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);

    println!("🔧 Max concurrent slots: {}", max_concurrent);

    // Create concurrent processor
    let concurrent_processor = ConcurrentSlotProcessor::new(
        monitor_arc.clone(),
        rpc_url.clone(),
        Some(max_concurrent),
    );

    // Create selective monitor for advanced filtering
    let selective_monitor = if use_config_dir {
        // Load all monitor configs to build selective monitoring rules
        let mut all_monitors = Vec::new();

        // Read all monitor JSON files
        let monitor_dir = std::path::Path::new("config/monitors");
        if monitor_dir.exists() {
            for entry in std::fs::read_dir(monitor_dir).context("Failed to read monitors directory")? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    let content = std::fs::read_to_string(&path)
                        .with_context(|| format!("Failed to read monitor file: {:?}", path))?;
                    let monitors: Vec<serde_json::Value> = serde_json::from_str(&content)
                        .with_context(|| format!("Failed to parse monitor file: {:?}", path))?;
                    all_monitors.extend(monitors);
                }
            }
        }

        match SelectiveMonitor::from_monitor_configs(rpc_url.clone(), &all_monitors) {
            Ok(monitor) => {
                println!("✅ Selective monitoring enabled - intelligent slot filtering");
                Some(Arc::new(monitor))
            }
            Err(e) => {
                println!("⚠️  Failed to create selective monitor: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Create YU-focused filter if optimization_yu_focused.json exists
    let yu_filter = if Path::new("config/optimization_yu_focused.json").exists() {
        println!("🎯 YU-focused mode enabled - ONLY monitoring YU token transactions");
        Some(Arc::new(YuFocusedFilter::new(rpc_url.clone())))
    } else {
        None
    };

    // Create pre-filter if optimization config exists (fallback)
    let pre_filter = if yu_filter.is_none() && selective_monitor.is_none() && Path::new("config/optimization.json").exists() {
        match SlotPreFilter::from_config_file(rpc_url.clone(), "config/optimization.json") {
            Ok(filter) => {
                println!("✅ Pre-filtering enabled - will skip irrelevant slots");
                Some(Arc::new(filter))
            }
            Err(e) => {
                println!("⚠️  Failed to load pre-filter config: {}", e);
                None
            }
        }
    } else {
        None
    };

    loop {
        // Get the latest slot from RPC
        let latest_slot = match rpc_client.get_slot().await {
            Ok(slot) => slot,
            Err(e) => {
                consecutive_errors += 1;
                error!("Failed to get current slot: {}", e);

                if consecutive_errors > 5 {
                    return Err(anyhow::anyhow!("Too many consecutive errors getting slot"));
                }

                sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        consecutive_errors = 0;

        // Check if we're catching up or monitoring live
        let slots_behind = latest_slot.saturating_sub(current_slot);
        let is_catching_up = slots_behind > 10;

        if is_catching_up {
            // Process slots in batches when catching up
            let batch_size = std::cmp::min(slots_behind, 500);
            let end_slot = current_slot + batch_size - 1;

            let slots_to_process: Vec<u64> = (current_slot..=end_slot).collect();

            // Apply YU-focused filter first (most restrictive)
            let slots_to_process = if let Some(ref yu_filter) = yu_filter {
                println!("🎯 YU-focused filtering {} slots...", slots_to_process.len());
                match yu_filter.filter_yu_slots(slots_to_process).await {
                    Ok(yu_slots) => {
                        if yu_slots.is_empty() {
                            println!("  ⚠️  No YU token activity found in this batch");
                        } else {
                            println!("  ✅ Found {} slots with YU token activity ({:.1}% of batch)",
                                     yu_slots.len(),
                                     yu_slots.len() as f64 / batch_size as f64 * 100.0
                            );
                        }
                        yu_slots
                    }
                    Err(e) => {
                        println!("  ⚠️  YU filter failed: {}, processing all slots", e);
                        (current_slot..=end_slot).collect()
                    }
                }
            } else if let Some(ref selective_monitor) = selective_monitor {
                println!("🎯 Applying selective monitoring to {} slots...", slots_to_process.len());
                match selective_monitor.should_monitor_slots(&slots_to_process).await {
                    Ok(filtered) => {
                        if filtered.is_empty() {
                            println!("  ⏸️  Low activity detected - reducing monitoring frequency");
                        } else {
                            println!("  ✅ Found {} slots to monitor (skipping {})",
                                     filtered.len(),
                                     batch_size as usize - filtered.len()
                            );
                        }

                        // Get activity stats
                        if let Ok(stats) = selective_monitor.get_activity_stats().await {
                            if stats.consecutive_empty_slots > 0 {
                                println!("  📊 {} consecutive empty slots", stats.consecutive_empty_slots);
                            }
                            if let Some(token) = stats.most_active_token {
                                println!("  🔥 Most active: {}...", &token[..8]);
                            }
                        }

                        filtered
                    }
                    Err(e) => {
                        println!("  ⚠️  Selective monitor failed: {}, falling back to pre-filter", e);
                        if let Some(ref pre_filter) = pre_filter {
                            pre_filter.filter_relevant_slots(slots_to_process).await
                                .unwrap_or_else(|_| (current_slot..=end_slot).collect())
                        } else {
                            (current_slot..=end_slot).collect()
                        }
                    }
                }
            } else if let Some(ref pre_filter) = pre_filter {
                println!("🔍 Pre-filtering {} slots...", slots_to_process.len());
                match pre_filter.filter_relevant_slots(slots_to_process).await {
                    Ok(filtered) => {
                        println!("  ✅ Found {} potentially relevant slots (skipping {})",
                                 filtered.len(),
                                 batch_size as usize - filtered.len()
                        );
                        filtered
                    }
                    Err(e) => {
                        println!("  ⚠️  Pre-filter failed: {}, processing all slots", e);
                        (current_slot..=end_slot).collect()
                    }
                }
            } else {
                slots_to_process
            };

            if slots_to_process.is_empty() {
                // No relevant slots in this batch, skip ahead
                println!("  ⏩ Skipping batch - no relevant transactions");
                current_slot = end_slot + 1;

                // Important: Update checkpoint even when skipping
                total_scanned += batch_size as u64;
                let checkpoint = SlotCheckpoint::new(end_slot, total_scanned, total_matched);
                if let Err(e) = checkpoint.save(CHECKPOINT_FILE) {
                    error!("Failed to save checkpoint: {}", e);
                } else {
                    let new_latest = rpc_client.get_slot().await.unwrap_or(latest_slot);
                    let new_slots_behind = new_latest.saturating_sub(current_slot);
                    println!("\n💾 Checkpoint saved at slot {} (catching up: {} slots behind)",
                             end_slot,
                             new_slots_behind.to_string().bright_yellow()
                    );
                    println!("📊 Progress: {} slots scanned (skipped), {} matches found",
                             total_scanned,
                             total_matched.to_string().bright_green()
                    );
                    println!("⏱️  Current slot: {}, Latest slot: {}\n", current_slot, new_latest);
                }
                continue;
            }

            println!("⚡ Processing {} relevant slots from batch ({} slots behind)...",
                     slots_to_process.len(),
                     slots_behind.to_string().bright_yellow()
            );

            // Process only the relevant slots
            let start = *slots_to_process.first().unwrap();
            let end = *slots_to_process.last().unwrap();

            // Process batch concurrently
            match concurrent_processor.process_slots(start, end).await {
                Ok(results) => {
                    let mut batch_matched = 0;
                    let mut batch_processed = 0;

                    for result in &results {
                        if result.success {
                            batch_processed += 1;
                            let matched_count = result.matched_transactions.len();

                            if matched_count > 0 {
                                println!("  ✅ Slot {} - Found {} matching transactions",
                                         result.slot,
                                         matched_count.to_string().bright_green()
                                );

                                // Show matched transactions
                                for tx in &result.matched_transactions {
                                    println!("    📌 {} - Filters: {}",
                                             &tx.transaction.signature[..20],
                                             tx.matched_filters.join(", ").bright_yellow()
                                    );
                                }


                                batch_matched += matched_count;
                            }
                        }

                        total_scanned += 1;
                        total_matched += result.matched_transactions.len() as u64;
                    }

                    // Update current slot
                    current_slot = end_slot + 1;

                    // Save checkpoint after batch
                    let checkpoint = SlotCheckpoint::new(end_slot, total_scanned, total_matched);
                    if let Err(e) = checkpoint.save(CHECKPOINT_FILE) {
                        error!("Failed to save checkpoint: {}", e);
                    } else {
                        println!("\n💾 Checkpoint saved at slot {} (catching up: {} slots behind)",
                                 end_slot,
                                 latest_slot.saturating_sub(current_slot).to_string().bright_yellow()
                        );
                        println!("📊 Batch summary: {} slots processed, {} matches found",
                                 batch_processed,
                                 batch_matched.to_string().bright_green()
                        );
                        println!("📊 Total progress: {} slots scanned, {} matches found\n",
                                 total_scanned,
                                 total_matched.to_string().bright_green()
                        );

                        // Update selective monitor with activity data if matches found
                        if let (Some(selective_monitor), true) = (&selective_monitor, batch_matched > 0) {
                            // Collect token activities from matched transactions
                            let mut token_activities = Vec::new();
                            for result in &results {
                                for tx in &result.matched_transactions {
                                    for change in &tx.transaction.token_balance_changes {
                                        if change.change.abs() > 0.0 {
                                            token_activities.push((change.mint.clone(), change.change.abs()));
                                        }
                                    }
                                }
                            }

                            if !token_activities.is_empty() {
                                let _ = selective_monitor.update_activity(end_slot, token_activities).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to process batch: {}", e);
                    // Fall back to sequential processing
                    current_slot = end_slot + 1;
                }
            }
        } else {
            // Process slots individually when monitoring live
            while current_slot <= latest_slot {
                println!("⚡ Monitoring slot {} (live mode)...", current_slot);

                match monitor_arc.monitor_slot(current_slot).await {
                    Ok(matched_transactions) => {
                        let matched_count = matched_transactions.len();
                        if matched_count > 0 {
                            println!("  ✅ Found {} matching transactions", matched_count.to_string().bright_green());

                            // Show matched transactions
                            for tx in &matched_transactions {
                                println!("    📌 {} - Filters: {}",
                                         &tx.transaction.signature[..20],
                                         tx.matched_filters.join(", ").bright_yellow()
                                );
                            }

                            total_matched += matched_count as u64;
                        }

                        total_scanned += 1;

                        // Determine checkpoint frequency based on whether we're catching up
                        let is_catching_up = current_slot < latest_slot;
                        let checkpoint_interval = if is_catching_up { 500 } else { 10 };

                        // Save checkpoint based on interval
                        if total_scanned % checkpoint_interval == 0 {
                            let checkpoint = SlotCheckpoint::new(current_slot, total_scanned, total_matched);
                            if let Err(e) = checkpoint.save(CHECKPOINT_FILE) {
                                error!("Failed to save checkpoint: {}", e);
                            } else {
                                if is_catching_up {
                                    println!("  💾 Checkpoint saved at slot {} (catching up: {} slots behind)",
                                             current_slot,
                                             (latest_slot - current_slot).to_string().bright_yellow()
                                    );
                                } else {
                                    println!("  💾 Checkpoint saved at slot {} (live monitoring)", current_slot);
                                }
                            }

                            println!("  📊 Progress: {} slots scanned, {} matches found",
                                     total_scanned,
                                     total_matched.to_string().bright_green()
                            );
                        }
                    },
                    Err(e) => {
                        error!("Failed to monitor slot {}: {}", current_slot, e);
                    }
                }

                current_slot += 1;
            }
        }

        // Wait before checking for new slots
        sleep(Duration::from_millis(400)).await;
    }
}

fn generate_config(output: &str) -> Result<()> {
    println!("{}", "📝 Generating Example Filter Configuration".bright_cyan().bold());
    println!("{}", "=========================================".bright_cyan());

    let filters = create_example_filter_config();
    save_filter_config(&filters, output)?;

    println!("✅ Generated {} filters", filters.len());
    println!("💾 Saved to: {}", output.bright_green());

    println!("\nExample filters include:");
    for filter in &filters {
        println!("  • {} ({})", filter.name.bright_yellow(), filter.id);
    }

    println!("\n📌 To use this configuration:");
    println!("   cargo run --bin monitor_with_filters -- --filter-config {} monitor <SLOTS>", output);
    println!("\n📌 Or create config/monitors/ and config/alerts/ directories for automatic loading");

    Ok(())
}

async fn test_slot(
    slot: u64,
    filter_config: Option<String>,
    rpc_url: Option<String>,
) -> Result<()> {
    println!("{}", "🧪 Testing Filters on Single Slot".bright_cyan().bold());
    println!("{}", "=================================".bright_cyan());

    let rpc_url = rpc_url.unwrap_or_else(|| {
        env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string())
    });

    println!("📊 Testing slot: {}", slot);
    println!("🌐 RPC: {}", rpc_url.bright_blue());

    // Check if config directory exists
    let config_dir = std::path::Path::new("config");
    let use_config_dir = config_dir.exists() && config_dir.is_dir();

    let monitor = if use_config_dir {
        FilteredTransactionMonitor::from_config_dir(rpc_url, "config").await?
    } else {
        FilteredTransactionMonitor::new(rpc_url, filter_config).await?
    };

    match monitor.monitor_slot(slot).await {
        Ok(matched_transactions) => {
            println!("\n✅ Test completed successfully");
            println!("Found {} matching transactions", matched_transactions.len());

            for (i, tx) in matched_transactions.iter().enumerate() {
                println!("\n{}. Transaction {}", i + 1, &tx.transaction.signature[..44]);
                println!("   Matched filters: {}", tx.matched_filters.join(", ").bright_yellow());
                println!("   Success: {}", tx.transaction.success);
                println!("   Fee: {} SOL", tx.transaction.fee as f64 / 1_000_000_000.0);

                // Show token changes if any
                for change in &tx.transaction.token_balance_changes {
                    if change.change.abs() > 0.0 {
                        println!("   Token change: {:+.2} ({})",
                                 change.change,
                                 &change.mint[..8]
                        );
                    }
                }
            }
        },
        Err(e) => {
            println!("❌ Test failed: {}", e);
        }
    }

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct SlotCheckpoint {
    last_processed_slot: u64,
    timestamp: u64,
    total_slots_processed: u64,
    total_matches_found: u64,
}

impl SlotCheckpoint {
    fn new(slot: u64, total_slots: u64, total_matches: u64) -> Self {
        Self {
            last_processed_slot: slot,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            total_slots_processed: total_slots,
            total_matches_found: total_matches,
        }
    }

    fn load(path: &str) -> Result<Option<Self>> {
        if Path::new(path).exists() {
            let content = fs::read_to_string(path)?;
            let checkpoint: SlotCheckpoint = serde_json::from_str(&content)?;
            Ok(Some(checkpoint))
        } else {
            Ok(None)
        }
    }

    fn save(&self, path: &str) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}