use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use solana_client::rpc_config::RpcBlockConfig;
use solana_transaction_status::{TransactionDetails, UiTransactionEncoding};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::rpc_client_with_failover::RpcClientWithFailover;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreFilterConfig {
    pub monitored_programs: Vec<String>,
    pub monitored_tokens: Vec<String>,
}

/// Pre-filter slots to skip those that definitely don't contain relevant transactions
pub struct SlotPreFilter {
    rpc_client: Arc<RpcClientWithFailover>,
    monitored_addresses: HashSet<String>,
}

impl SlotPreFilter {
    pub fn new(rpc_url: String, config: PreFilterConfig) -> Self {
        let rpc_client = Arc::new(RpcClientWithFailover::new(rpc_url));
        
        // Combine all monitored addresses into a single set for fast lookup
        let mut monitored_addresses = HashSet::new();
        for addr in config.monitored_programs {
            monitored_addresses.insert(addr);
        }
        for addr in config.monitored_tokens {
            monitored_addresses.insert(addr);
        }
        
        info!("Initialized slot pre-filter with {} monitored addresses", monitored_addresses.len());
        
        Self {
            rpc_client,
            monitored_addresses,
        }
    }

    /// Load pre-filter config from file
    pub fn from_config_file(rpc_url: String, config_path: &str) -> Result<Self> {
        let config_str = std::fs::read_to_string(config_path)
            .context("Failed to read optimization config")?;
        
        let config: serde_json::Value = serde_json::from_str(&config_str)
            .context("Failed to parse optimization config")?;
        
        let pre_filters = config.get("pre_filters")
            .ok_or_else(|| anyhow::anyhow!("Missing pre_filters in config"))?;
        
        let monitored_programs: Vec<String> = pre_filters
            .get("monitored_programs")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect())
            .unwrap_or_default();
        
        let monitored_tokens: Vec<String> = pre_filters
            .get("monitored_tokens")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect())
            .unwrap_or_default();
        
        let config = PreFilterConfig {
            monitored_programs,
            monitored_tokens,
        };
        
        Ok(Self::new(rpc_url, config))
    }

    /// Check if a slot might contain relevant transactions
    pub async fn slot_might_contain_matches(&self, slot: u64) -> Result<bool> {
        // Get block with account keys and signatures
        let config = RpcBlockConfig {
            encoding: Some(UiTransactionEncoding::JsonParsed),
            transaction_details: Some(TransactionDetails::Signatures),
            rewards: Some(false),
            commitment: None,
            max_supported_transaction_version: Some(0),
        };
        
        match self.rpc_client.get_block_with_config(slot, config).await {
            Ok(block) => {
                if let Some(transactions) = block.transactions {
                    // Quick scan through account keys
                    for tx in transactions {
                        // Get account keys from the transaction
                        match &tx.transaction {
                            solana_transaction_status::EncodedTransaction::Json(json_tx) => {
                                // For JSON encoding, check account keys in the message
                                match &json_tx.message {
                                    solana_transaction_status::UiMessage::Parsed(parsed) => {
                                        for account in &parsed.account_keys {
                                            if self.monitored_addresses.contains(&account.pubkey) {
                                                debug!("Slot {} contains monitored address: {}", slot, account.pubkey);
                                                return Ok(true);
                                            }
                                        }
                                    }
                                    solana_transaction_status::UiMessage::Raw(raw) => {
                                        for key in &raw.account_keys {
                                            if self.monitored_addresses.contains(key) {
                                                debug!("Slot {} contains monitored address: {}", slot, key);
                                                return Ok(true);
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                // For other encodings, skip for now
                                continue;
                            }
                        }
                    }
                }
                debug!("Slot {} does not contain any monitored addresses", slot);
                Ok(false)
            }
            Err(e) => {
                debug!("Failed to get block {}: {}", slot, e);
                Ok(false) // Assume slot is empty or doesn't exist
            }
        }
    }

    /// Batch check multiple slots
    pub async fn filter_relevant_slots(&self, slots: Vec<u64>) -> Result<Vec<u64>> {
        let mut relevant_slots = Vec::new();
        let start_time = std::time::Instant::now();
        
        info!("Pre-filtering {} slots with {} monitored addresses", 
            slots.len(), self.monitored_addresses.len());
        
        // Log first few monitored addresses for debugging
        let addr_sample: Vec<&str> = self.monitored_addresses.iter()
            .take(5)
            .map(|s| s.as_str())
            .collect();
        info!("Sample monitored addresses: {:?}", addr_sample);
        
        // Track statistics
        let mut total_txs_scanned = 0u64;
        let mut total_blocks_with_txs = 0u64;
        
        // Process in smaller batches to avoid overwhelming the RPC
        for (chunk_idx, chunk) in slots.chunks(20).enumerate() {
            let mut handles = vec![];
            let is_first_chunk = chunk_idx == 0;
            let first_slot_in_chunk = chunk.first().copied().unwrap_or(0);
            
            for &slot in chunk {
                let rpc_client = self.rpc_client.clone();
                let monitored = self.monitored_addresses.clone();
                
                let handle = tokio::spawn(async move {
                    let config = RpcBlockConfig {
                        encoding: Some(UiTransactionEncoding::JsonParsed),
                        transaction_details: Some(TransactionDetails::Full),
                        rewards: Some(false),
                        commitment: None,
                        max_supported_transaction_version: Some(0),
                    };
                    
                    match rpc_client.get_block_with_config(slot, config).await {
                        Ok(block) => {
                            if let Some(transactions) = block.transactions {
                                let tx_count = transactions.len();
                                if tx_count > 0 {
                                    // Log first slot with transactions for debugging
                                    if is_first_chunk && slot == first_slot_in_chunk {
                                        info!("Slot {} has {} transactions, checking for monitored addresses...", slot, tx_count);
                                    }
                                }
                                
                                for tx in transactions {
                                    // Check transaction metadata for token accounts
                                    if let Some(meta) = &tx.meta {
                                        // Check pre/post token balances for monitored tokens
                                        match &meta.pre_token_balances {
                                            solana_transaction_status::option_serializer::OptionSerializer::Some(balances) => {
                                                for balance in balances {
                                                    // Log token mints seen in first few slots
                                                    if is_first_chunk && balance.ui_token_amount.ui_amount.unwrap_or(0.0) > 0.0 {
                                                        debug!("Token mint in slot {}: {} (amount: {})", 
                                                            slot, balance.mint, balance.ui_token_amount.ui_amount.unwrap_or(0.0));
                                                    }
                                                    if monitored.contains(&balance.mint) {
                                                        info!("✅ Found monitored token {} in slot {} (pre-balance)", balance.mint, slot);
                                                        return (slot, true, tx_count);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                        match &meta.post_token_balances {
                                            solana_transaction_status::option_serializer::OptionSerializer::Some(balances) => {
                                                for balance in balances {
                                                    if monitored.contains(&balance.mint) {
                                                        info!("✅ Found monitored token {} in slot {} (post-balance)", balance.mint, slot);
                                                        return (slot, true, tx_count);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    
                                    // Check account keys in transaction
                                    match &tx.transaction {
                                        solana_transaction_status::EncodedTransaction::Json(json_tx) => {
                                            match &json_tx.message {
                                                solana_transaction_status::UiMessage::Parsed(parsed) => {
                                                    for account in &parsed.account_keys {
                                                        if monitored.contains(&account.pubkey) {
                                                            info!("✅ Found monitored program {} in slot {} (parsed)", account.pubkey, slot);
                                                            return (slot, true, tx_count);
                                                        }
                                                    }
                                                }
                                                solana_transaction_status::UiMessage::Raw(raw) => {
                                                    for key in &raw.account_keys {
                                                        if monitored.contains(key) {
                                                            info!("✅ Found monitored program {} in slot {} (raw)", key, slot);
                                                            return (slot, true, tx_count);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        _ => continue,
                                    }
                                }
                            }
                            (slot, false, 0)
                        }
                        Err(e) => {
                            debug!("Failed to fetch slot {}: {}", slot, e);
                            (slot, false, 0)
                        },
                    }
                });
                
                handles.push(handle);
            }
            
            // Collect results
            for handle in handles {
                if let Ok((slot, is_relevant, tx_count)) = handle.await {
                    if tx_count > 0 {
                        total_blocks_with_txs += 1;
                        total_txs_scanned += tx_count as u64;
                    }
                    if is_relevant {
                        relevant_slots.push(slot);
                    }
                }
            }
        }
        
        let elapsed = start_time.elapsed();
        info!("Pre-filter completed in {:.2}s: {} relevant out of {} slots ({:.1} slots/sec)", 
            elapsed.as_secs_f64(),
            relevant_slots.len(), 
            slots.len(),
            slots.len() as f64 / elapsed.as_secs_f64()
        );
        info!("  Scanned {} transactions in {} non-empty blocks", 
            total_txs_scanned, total_blocks_with_txs);
        
        if relevant_slots.is_empty() && !slots.is_empty() {
            warn!("No relevant slots found in batch {}..{}", 
                slots.first().unwrap(), slots.last().unwrap());
            if total_blocks_with_txs == 0 {
                warn!("  Note: All slots in this batch were empty (no transactions)");
            } else {
                warn!("  Scanned {} transactions but found no matches for monitored addresses", total_txs_scanned);
                warn!("  Looking for: YU token ({}...), Raydium ({}...), etc.", 
                    &self.monitored_addresses.iter().find(|a| a.starts_with("YU")).map(|s| &s[..8]).unwrap_or("N/A"),
                    &self.monitored_addresses.iter().find(|a| a.starts_with("675")).map(|s| &s[..8]).unwrap_or("N/A"));
            }
        }
        
        Ok(relevant_slots)
    }
}