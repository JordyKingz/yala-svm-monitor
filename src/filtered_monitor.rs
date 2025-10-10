use anyhow::{Result, Context};
use solana_client::rpc_client::RpcClient;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

use crate::filter_engine::{FilterEngine, FilterConfig, Action, AlertSeverity, create_yuya_mint_filters};
use crate::telegram_notifier::TelegramNotifier;
use crate::discord_notifier::DiscordNotifier;
use crate::slack_notifier::SlackNotifier;
use crate::transaction_extractor::{TransactionExtractor, ExtractedTransaction};
use crate::notifications::NotificationManager;
use crate::config_manager::ConfigManager;

pub struct FilteredTransactionMonitor {
    rpc_client: Arc<RpcClient>,
    pub filter_engine: Arc<FilterEngine>,
    telegram_notifier: Option<Arc<TelegramNotifier>>,
    slack_notifier: Option<Arc<SlackNotifier>>,
    notification_manager: Arc<RwLock<NotificationManager>>,
    transaction_extractor: Arc<TransactionExtractor>,
    storage: Arc<RwLock<TransactionStorage>>,
    config_manager: Option<Arc<ConfigManager>>,
}

#[derive(Debug, Clone)]
pub struct StoredTransaction {
    pub transaction: ExtractedTransaction,
    pub matched_filters: Vec<String>,
    pub stored_at: DateTime<Utc>,
    pub collection: String,
}

pub struct TransactionStorage {
    collections: HashMap<String, Vec<StoredTransaction>>,
}

impl TransactionStorage {
    pub fn new() -> Self {
        Self {
            collections: HashMap::new(),
        }
    }
    
    pub fn store_transaction(
        &mut self,
        transaction: ExtractedTransaction,
        collection: &str,
        filter_id: &str,
    ) {
        let stored = StoredTransaction {
            transaction,
            matched_filters: vec![filter_id.to_string()],
            stored_at: Utc::now(),
            collection: collection.to_string(),
        };
        
        self.collections
            .entry(collection.to_string())
            .or_insert_with(Vec::new)
            .push(stored);
    }
    
    pub fn get_collection(&self, collection: &str) -> Option<&Vec<StoredTransaction>> {
        self.collections.get(collection)
    }
    
    pub fn get_all_collections(&self) -> Vec<(String, usize)> {
        self.collections
            .iter()
            .map(|(name, txs)| (name.clone(), txs.len()))
            .collect()
    }
}

impl FilteredTransactionMonitor {
    pub async fn new(
        rpc_url: String,
        filter_config_path: Option<String>,
    ) -> Result<Self> {
        let rpc_client = Arc::new(RpcClient::new(rpc_url.clone()));
        
        // Load filters
        let filters = if let Some(path) = filter_config_path {
            FilterEngine::from_json_file(&path)?
        } else {
            // Use default YUYA mint filters
            let yuya_address = std::env::var("YU_TOKEN_ADDRESS")
                .unwrap_or_else(|_| "YUYAiJo8KVbnc6Fb6h3MnH2VGND4uGWDH4iLnw7DLEu".to_string());
            let default_filters = create_yuya_mint_filters(&yuya_address);
            FilterEngine::new(default_filters)
        };
        
        let filter_engine = Arc::new(filters);
        
        // Setup Telegram if credentials are available
        let telegram_notifier = match (
            std::env::var("TELEGRAM_BOT_TOKEN"),
            std::env::var("TELEGRAM_CHAT_ID")
        ) {
            (Ok(token), Ok(chat_id)) => {
                info!("Telegram notifications enabled");
                let notifier = TelegramNotifier::new(token, chat_id);
                
                // Send test message
                if let Err(e) = notifier.send_message("🚀 Solana transaction monitor started! Filters are active.").await {
                    warn!("Failed to send Telegram test message: {}", e);
                }
                
                Some(Arc::new(notifier))
            },
            _ => {
                info!("Telegram notifications disabled (credentials not set)");
                None
            }
        };
        
        // Setup Slack if webhook URL is available
        // let slack_notifier = match std::env::var("SLACK_WEBHOOK_URL") {
        //     Ok(webhook_url) => {
        //         info!("Slack notifications enabled");
        //         match SlackNotifier::from_url(webhook_url).send_simple_message("🚀 Solana transaction monitor started! Filters are active.").await {
        //             Ok(_) => info!("Slack test message sent successfully"),
        //             Err(e) => warn!("Failed to send Slack test message: {}", e),
        //         }
        //
        //         match SlackNotifier::new() {
        //             Ok(notifier) => Some(Arc::new(notifier)),
        //             Err(e) => {
        //                 warn!("Failed to create Slack notifier: {}", e);
        //                 None
        //             }
        //         }
        //     },
        //     _ => {
        //         info!("Slack notifications disabled (SLACK_WEBHOOK_URL not set)");
        //         None
        //     }
        // };
        
        let transaction_extractor = Arc::new(TransactionExtractor::new(rpc_url));
        let notification_manager = Arc::new(RwLock::new(NotificationManager::new()));
        let storage = Arc::new(RwLock::new(TransactionStorage::new()));
        
        Ok(Self {
            rpc_client,
            filter_engine,
            telegram_notifier,
            slack_notifier: None,
            notification_manager,
            transaction_extractor,
            storage,
            config_manager: None,
        })
    }
    
    /// Create monitor from config directory
    pub async fn from_config_dir(
        rpc_url: String,
        config_dir: impl AsRef<std::path::Path>,
    ) -> Result<Self> {
        let rpc_client = Arc::new(RpcClient::new(rpc_url.clone()));
        
        // Load configurations
        let mut config_manager = ConfigManager::new(config_dir);
        config_manager.load_all()?;
        
        // Get filters with resolved alerts
        let filters = config_manager.get_filters_with_alerts()?;
        let filter_engine = Arc::new(FilterEngine::new(filters));
        let config_manager = Arc::new(config_manager);
        
        // Setup Telegram if credentials are available
        let telegram_notifier = match (
            std::env::var("TELEGRAM_BOT_TOKEN"),
            std::env::var("TELEGRAM_CHAT_ID")
        ) {
            (Ok(token), Ok(chat_id)) => {
                info!("Telegram notifications enabled");
                let notifier = TelegramNotifier::new(token, chat_id);
                
                // Send test message
                if let Err(e) = notifier.send_message("🚀 Solana transaction monitor started! Filters are active.").await {
                    warn!("Failed to send Telegram test message: {}", e);
                }
                
                Some(Arc::new(notifier))
            },
            _ => {
                info!("Telegram notifications disabled (credentials not set)");
                None
            }
        };
        
        // Setup Slack if webhook URL is available
        let slack_notifier = match std::env::var("SLACK_WEBHOOK_URL") {
            Ok(webhook_url) => {
                info!("Slack notifications enabled");

                match SlackNotifier::new() {
                    Ok(notifier) => Some(Arc::new(notifier)),
                    Err(e) => {
                        warn!("Failed to create Slack notifier: {}", e);
                        None
                    }
                }
            },
            _ => {
                info!("Slack notifications disabled (SLACK_WEBHOOK_URL not set)");
                None
            }
        };
        
        let transaction_extractor = Arc::new(TransactionExtractor::new(rpc_url));
        let notification_manager = Arc::new(RwLock::new(NotificationManager::new()));
        let storage = Arc::new(RwLock::new(TransactionStorage::new()));
        
        Ok(Self {
            rpc_client,
            filter_engine,
            telegram_notifier,
            slack_notifier,
            notification_manager,
            transaction_extractor,
            storage,
            config_manager: Some(config_manager),
        })
    }
    
    pub async fn monitor_slot(&self, slot: u64) -> Result<Vec<StoredTransaction>> {
        info!("Monitoring slot {} with filters", slot);
        
        let transactions = self.transaction_extractor
            .extract_from_slot(slot)
            .await
            .context("Failed to extract transactions")?;
        
        info!("Extracted {} transactions from slot {}", transactions.len(), slot);
        
        let mut stored_transactions = Vec::new();
        
        for transaction in transactions {
            let matched_filters = self.filter_engine.evaluate_transaction(&transaction);
            
            if !matched_filters.is_empty() {
                let original_count = matched_filters.len();
                
                // Deduplicate filters by category to only keep the highest priority one
                let deduplicated_filters = self.deduplicate_filters(matched_filters);
                
                info!(
                    "Transaction {} matched {} filter(s) (deduplicated from {})",
                    transaction.signature,
                    deduplicated_filters.len(),
                    original_count
                );
                
                // Process actions for each matched filter
                for matched_filter in &deduplicated_filters {
                    for action in &matched_filter.actions {
                        if let Err(e) = self.process_action(
                            action,
                            &transaction,
                            matched_filter,
                        ).await {
                            error!("Failed to process action: {}", e);
                        }
                    }
                }
                
                // Create a stored transaction record
                let stored = StoredTransaction {
                    transaction: transaction.clone(),
                    matched_filters: deduplicated_filters.iter()
                        .map(|f| f.filter_id.clone())
                        .collect(),
                    stored_at: Utc::now(),
                    collection: "filtered".to_string(),
                };
                stored_transactions.push(stored);
            }
        }
        
        Ok(stored_transactions)
    }
    
    async fn process_action(
        &self,
        action: &Action,
        transaction: &ExtractedTransaction,
        matched_filter: &crate::filter_engine::MatchedFilter,
    ) -> Result<()> {
        match action {
            Action::Alert { severity, channels } => {
                for channel in channels {
                    match channel.as_str() {
                        "telegram" => {
                            if let Some(telegram) = &self.telegram_notifier {
                                // Look for telegram template if config manager is available
                                let template = if let Some(config_mgr) = &self.config_manager {
                                    self.find_telegram_template(config_mgr, &matched_filter.filter_id, transaction)
                                } else {
                                    None
                                };
                                
                                if let Some((title, body)) = template {
                                    telegram.send_custom_message(&title, &body).await?;
                                } else {
                                    telegram.send_alert(transaction, matched_filter, severity).await?;
                                }
                            }
                        },
                        "database" => {
                            // Store in notification manager
                            let mut nm = self.notification_manager.write().await;
                            nm.add_notification(
                                &matched_filter.filter_name,
                                &format!("Transaction {} matched filter", transaction.signature),
                                match severity {
                                    AlertSeverity::Low => crate::notifications::NotificationLevel::Info,
                                    AlertSeverity::Medium => crate::notifications::NotificationLevel::Warning,
                                    AlertSeverity::High | AlertSeverity::Critical => {
                                        crate::notifications::NotificationLevel::Error
                                    },
                                },
                            );
                        },
                        "slack" => {
                            if let Some(slack) = &self.slack_notifier {
                                // Look for slack template if config manager is available
                                let template = if let Some(config_mgr) = &self.config_manager {
                                    self.find_slack_template(config_mgr, &matched_filter.filter_id, transaction)
                                } else {
                                    None
                                };
                                
                                if let Some((title, body)) = template {
                                    slack.send_simple_message(&format!("{}\n\n{}", title, body)).await?;
                                } else {
                                    // Send formatted transaction alert
                                    let amount = transaction.token_balance_changes.first()
                                        .map(|change| change.change);
                                    let token = transaction.token_balance_changes.first()
                                        .map(|change| change.mint.as_str());
                                    
                                    slack.send_transaction_alert(
                                        &format!("🚨 {} - {:?}", matched_filter.filter_name, severity),
                                        &transaction.signature,
                                        transaction.slot,
                                        amount,
                                        token,
                                        vec![
                                            ("Filter".to_string(), matched_filter.filter_name.clone()),
                                            ("Success".to_string(), transaction.success.to_string()),
                                            ("Fee".to_string(), format!("{} lamports", transaction.fee)),
                                        ],
                                    ).await?;
                                }
                            }
                        },
                        _ => {
                            warn!("Unknown notification channel: {}", channel);
                        }
                    }
                }
            },
            
            Action::Store { collection } => {
                let mut storage = self.storage.write().await;
                storage.store_transaction(
                    transaction.clone(),
                    collection,
                    &matched_filter.filter_id,
                );
                debug!("Stored transaction in collection: {}", collection);
            },
            
            Action::Webhook { url, method } => {
                if url.contains("discord.com/api/webhooks") {
                    // Handle Discord webhook
                    let discord = DiscordNotifier::new(url.clone());
                    
                    // Look for Discord template if config manager is available
                    let template = if let Some(config_mgr) = &self.config_manager {
                        self.find_discord_template(config_mgr, &matched_filter.filter_id, transaction)
                    } else {
                        None
                    };
                    
                    if let Err(e) = discord.send_transaction_alert(
                        transaction,
                        &matched_filter.filter_name,
                        template.as_ref(),
                    ).await {
                        error!("Failed to send Discord notification: {}", e);
                    }
                } else {
                    // Generic webhook
                    warn!("Generic webhook not yet implemented: {} {}", method, url);
                }
            },
            
            Action::Log { level, message } => {
                match level.as_str() {
                    "debug" => debug!("{}: {}", matched_filter.filter_name, message),
                    "info" => info!("{}: {}", matched_filter.filter_name, message),
                    "warn" => warn!("{}: {}", matched_filter.filter_name, message),
                    "error" => error!("{}: {}", matched_filter.filter_name, message),
                    _ => info!("{}: {}", matched_filter.filter_name, message),
                }
            },
        }
        
        Ok(())
    }
    
    pub async fn get_storage_summary(&self) -> HashMap<String, usize> {
        let storage = self.storage.read().await;
        storage.get_all_collections()
            .into_iter()
            .collect()
    }
    
    pub async fn get_stored_transactions(&self, collection: &str) -> Option<Vec<StoredTransaction>> {
        let storage = self.storage.read().await;
        storage.get_collection(collection).cloned()
    }
    
    /// Deduplicate filters to only keep the highest threshold match for each category
    fn deduplicate_filters(&self, matched_filters: Vec<crate::filter_engine::MatchedFilter>) -> Vec<crate::filter_engine::MatchedFilter> {
        use std::collections::HashMap;
        
        // Group filters by category prefix
        let mut filter_groups: HashMap<String, Vec<crate::filter_engine::MatchedFilter>> = HashMap::new();
        
        for filter in matched_filters {
            // Extract category from filter ID (e.g., "yuya_mint", "yuya_burn")
            let category = if filter.filter_id.contains("mint") {
                "yuya_mint".to_string()
            } else if filter.filter_id.contains("burn") {
                "yuya_burn".to_string()
            } else {
                // For non-mint/burn filters, use the full ID as category (no deduplication)
                filter.filter_id.clone()
            };
            
            filter_groups.entry(category).or_insert_with(Vec::new).push(filter);
        }
        
        // For each category, keep only the highest threshold filter
        let mut deduplicated = Vec::new();
        
        for (category, mut filters) in filter_groups {
            if filters.len() == 1 {
                deduplicated.push(filters.pop().unwrap());
            } else if category == "yuya_mint" || category == "yuya_burn" {
                // Sort by threshold (highest first)
                filters.sort_by(|a, b| {
                    let threshold_a = self.extract_threshold(&a.filter_id);
                    let threshold_b = self.extract_threshold(&b.filter_id);
                    threshold_b.partial_cmp(&threshold_a).unwrap()
                });
                
                // Take only the highest threshold filter
                if let Some(highest) = filters.into_iter().next() {
                    deduplicated.push(highest);
                }
            } else {
                // For other filter types, keep all
                deduplicated.extend(filters);
            }
        }
        
        deduplicated
    }
    
    /// Extract threshold value from filter ID (e.g., "yuya_mint_30m" -> 30.0)
    fn extract_threshold(&self, filter_id: &str) -> f64 {
        if filter_id.contains("30m") {
            30.0
        } else if filter_id.contains("10m") {
            10.0
        } else if filter_id.contains("1m") {
            1.0
        } else {
            0.0
        }
    }
    
    /// Find telegram template for a filter and format with transaction data
    fn find_telegram_template(
        &self, 
        config_mgr: &ConfigManager, 
        filter_id: &str,
        transaction: &ExtractedTransaction,
    ) -> Option<(String, String)> {
        // Get monitor config to find alert IDs
        if let Some(monitor) = config_mgr.loaded_monitors.get(filter_id) {
            // Look for telegram alerts
            for alert_id in &monitor.alerts {
                if let Some(alert) = config_mgr.get_alert(alert_id) {
                    if matches!(alert.trigger_type, crate::config_manager::AlertType::Telegram) {
                        // Convert transaction to JSON for template substitution
                        let transaction_json = serde_json::to_value(transaction).ok()?;
                        
                        let (title, body) = crate::config_manager::format_message(
                            &alert.config.message,
                            &transaction_json,
                        );
                        return Some((title, body));
                    }
                }
            }
        }
        None
    }
    
    /// Find slack template for a filter and format with transaction data
    fn find_slack_template(
        &self, 
        config_mgr: &ConfigManager, 
        filter_id: &str,
        transaction: &ExtractedTransaction,
    ) -> Option<(String, String)> {
        // Get monitor config to find alert IDs
        if let Some(monitor) = config_mgr.loaded_monitors.get(filter_id) {
            // Look for slack alerts
            for alert_id in &monitor.alerts {
                if let Some(alert) = config_mgr.get_alert(alert_id) {
                    if matches!(alert.trigger_type, crate::config_manager::AlertType::Slack) {
                        // Convert transaction to JSON for template substitution
                        let transaction_json = serde_json::to_value(transaction).ok()?;
                        
                        let (title, body) = crate::config_manager::format_message(
                            &alert.config.message,
                            &transaction_json,
                        );
                        return Some((title, body));
                    }
                }
            }
        }
        None
    }
    
    /// Find discord template for a filter and format with transaction data
    fn find_discord_template(
        &self, 
        config_mgr: &ConfigManager, 
        filter_id: &str,
        transaction: &ExtractedTransaction,
    ) -> Option<crate::config_manager::MessageTemplate> {
        // Get monitor config to find alert IDs
        if let Some(monitor) = config_mgr.loaded_monitors.get(filter_id) {
            // Look for discord alerts
            for alert_id in &monitor.alerts {
                if let Some(alert) = config_mgr.get_alert(alert_id) {
                    if matches!(alert.trigger_type, crate::config_manager::AlertType::Discord) {
                        // Convert transaction to JSON for template substitution
                        let transaction_json = serde_json::to_value(transaction).ok()?;
                        
                        let (title, body) = crate::config_manager::format_message(
                            &alert.config.message,
                            &transaction_json,
                        );
                        
                        return Some(crate::config_manager::MessageTemplate {
                            title,
                            body,
                        });
                    }
                }
            }
        }
        None
    }
}

// Helper to save filter configuration
pub fn save_filter_config(filters: &[FilterConfig], path: &str) -> Result<()> {
    let json = serde_json::to_string_pretty(filters)?;
    std::fs::write(path, json)?;
    info!("Saved {} filters to {}", filters.len(), path);
    Ok(())
}

// Create example filter configuration
pub fn create_example_filter_config() -> Vec<FilterConfig> {
    let yuya_address = std::env::var("YU_TOKEN_ADDRESS")
        .unwrap_or_else(|_| "YUYAiJo8KVbnc6Fb6h3MnH2VGND4uGWDH4iLnw7DLEu".to_string());
    
    let mut filters = create_yuya_mint_filters(&yuya_address);
    
    // Add a large YUYA DEX swap filter
    filters.push(FilterConfig {
        id: "yuya_dex_large_swap".to_string(),
        name: "Large YUYA DEX Swaps".to_string(),
        enabled: true,
        conditions: crate::filter_engine::ConditionSet {
            all_of: Some(vec![
                crate::filter_engine::Condition::ProgramInvoked {
                    program_id: "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK".to_string(), // Raydium CLMM
                },
                crate::filter_engine::Condition::TokenTransfer {
                    mint: Some(yuya_address.clone()),
                    operator: crate::filter_engine::ComparisonOperator::GreaterThan,
                    amount: 100_000.0, // 100k YUYA
                },
            ]),
            any_of: None,
            none_of: None,
        },
        actions: vec![
            Action::Alert {
                severity: AlertSeverity::High,
                channels: vec!["telegram".to_string(), "database".to_string()],
            },
            Action::Store {
                collection: "large_yuya_swaps".to_string(),
            },
        ],
    });
    
    filters
}