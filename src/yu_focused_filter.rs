use anyhow::{Result, Context};
use solana_client::rpc_config::RpcBlockConfig;
use solana_transaction_status::{TransactionDetails, UiTransactionEncoding};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::rpc_client_with_failover::RpcClientWithFailover;

/// YU-focused pre-filter that ONLY looks for YU token transactions
/// This is much more efficient than monitoring all DEX/USDC transactions
pub struct YuFocusedFilter {
    rpc_client: Arc<RpcClientWithFailover>,
    yu_token_mint: String,
    // Programs we care about when YU is involved
    monitored_programs_for_yu: Vec<String>,
}

impl YuFocusedFilter {
    pub fn new(rpc_url: String) -> Self {
        let rpc_client = Arc::new(RpcClientWithFailover::new(rpc_url));
        
        // YU token is our primary focus
        let yu_token_mint = "YUYAiJo8KVbnc6Fb6h3MnH2VGND4uGWDH4iLnw7DLEu".to_string();
        
        // Only care about these programs when YU is involved
        let monitored_programs_for_yu = vec![
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string(), // Raydium
            "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4".to_string(), // Jupiter V6
            "JUP4Fb2cqiRUcaTHdrPC8h2gNsA2ETXiPDD33WcGuJB".to_string(), // Jupiter V4
            "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc".to_string(), // Orca
            "6doghB248px58JSSwG4qejQ46kFMW4AMj7vzJnWZHNZn".to_string(), // LayerZero Bridge [OLD]
            "3fCoNdCEoEcERakCPM17NjLE9AocA86LMwRRWDpzjLVh".to_string(), // LayerZero Bridge [NEW]
        ];
        
        info!(
            "Initialized YU-focused filter - ONLY monitoring YU token ({}) interactions",
            &yu_token_mint[..8]
        );
        
        Self {
            rpc_client,
            yu_token_mint,
            monitored_programs_for_yu,
        }
    }
    
    /// Check if a slot contains YU token transactions
    /// This is the primary filter - if no YU, skip the slot entirely
    pub async fn slot_contains_yu_token(&self, slot: u64) -> Result<bool> {
        // Get block with full transaction details to check token balances
        let config = RpcBlockConfig {
            encoding: Some(UiTransactionEncoding::JsonParsed),
            transaction_details: Some(TransactionDetails::Full),
            rewards: Some(false),
            commitment: None,
            max_supported_transaction_version: Some(0),
        };
        
        match self.rpc_client.get_block_with_config(slot, config).await {
            Ok(block) => {
                if let Some(transactions) = block.transactions {
                    for tx in transactions {
                        // Check transaction metadata for YU token
                        if let Some(meta) = &tx.meta {
                            // Check pre-token balances
                            match &meta.pre_token_balances {
                                solana_transaction_status::option_serializer::OptionSerializer::Some(balances) => {
                                    for balance in balances {
                                        if balance.mint == self.yu_token_mint {
                                            debug!("Found YU token in slot {} (pre-balance)", slot);
                                            return Ok(true);
                                        }
                                    }
                                }
                                _ => {}
                            }
                            
                            // Check post-token balances
                            match &meta.post_token_balances {
                                solana_transaction_status::option_serializer::OptionSerializer::Some(balances) => {
                                    for balance in balances {
                                        if balance.mint == self.yu_token_mint {
                                            debug!("Found YU token in slot {} (post-balance)", slot);
                                            return Ok(true);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok(false)
            }
            Err(e) => {
                debug!("Failed to get block {}: {}", slot, e);
                Ok(false) // Assume slot doesn't exist or is empty
            }
        }
    }
    
    /// Batch check multiple slots for YU token activity
    pub async fn filter_yu_slots(&self, slots: Vec<u64>) -> Result<Vec<u64>> {
        let mut yu_slots = Vec::new();
        let start_time = std::time::Instant::now();
        
        info!("YU-focused filtering {} slots", slots.len());
        
        // Process in smaller batches
        for chunk in slots.chunks(20) {
            let mut handles = vec![];
            
            for &slot in chunk {
                let rpc_client = self.rpc_client.clone();
                let yu_mint = self.yu_token_mint.clone();
                
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
                                for tx in transactions {
                                    if let Some(meta) = &tx.meta {
                                        // Check pre-token balances
                                        match &meta.pre_token_balances {
                                            solana_transaction_status::option_serializer::OptionSerializer::Some(balances) => {
                                                for balance in balances {
                                                    if balance.mint == yu_mint {
                                                        return (slot, true);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                        
                                        // Check post-token balances
                                        match &meta.post_token_balances {
                                            solana_transaction_status::option_serializer::OptionSerializer::Some(balances) => {
                                                for balance in balances {
                                                    if balance.mint == yu_mint {
                                                        return (slot, true);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            (slot, false)
                        }
                        Err(_) => (slot, false),
                    }
                });
                
                handles.push(handle);
            }
            
            // Collect results
            for handle in handles {
                if let Ok((slot, has_yu)) = handle.await {
                    if has_yu {
                        yu_slots.push(slot);
                    }
                }
            }
        }
        
        let elapsed = start_time.elapsed();
        let yu_percentage = (yu_slots.len() as f64 / slots.len() as f64 * 100.0) as u32;
        
        info!(
            "YU filter completed in {:.2}s: {} out of {} slots contain YU token ({}%)",
            elapsed.as_secs_f64(),
            yu_slots.len(),
            slots.len(),
            yu_percentage
        );
        
        if yu_slots.is_empty() && !slots.is_empty() {
            warn!(
                "No YU token activity found in slots {}..{}",
                slots.first().unwrap(),
                slots.last().unwrap()
            );
        } else if !yu_slots.is_empty() {
            info!(
                "Found YU token activity in slots: {:?}",
                yu_slots.iter().take(5).collect::<Vec<_>>()
            );
        }
        
        Ok(yu_slots)
    }
    
    /// Get a summary of what we're monitoring
    pub fn get_filter_summary(&self) -> String {
        format!(
            "YU-focused filter:\n  \
             - Primary token: {} (YU)\n  \
             - Programs monitored (only when YU is involved): {}\n  \
             - Strategy: Skip ALL slots without YU token activity",
            self.yu_token_mint,
            self.monitored_programs_for_yu.len()
        )
    }
}

/// Optimized configuration for YU token monitoring
pub struct YuMonitorConfig {
    /// Only monitor slots with YU token activity
    pub yu_token_only: bool,
    
    /// Minimum YU amount to trigger monitoring (optional)
    pub min_yu_amount: Option<f64>,
    
    /// Skip monitoring if no YU activity for N slots
    pub skip_after_no_yu_slots: u32,
}

impl Default for YuMonitorConfig {
    fn default() -> Self {
        Self {
            yu_token_only: true,
            min_yu_amount: Some(100.0), // Only monitor if at least 100 YU involved
            skip_after_no_yu_slots: 100, // Skip monitoring after 100 slots without YU
        }
    }
}

/// Helper to check if we should monitor a transaction based on YU involvement
pub fn should_monitor_transaction(
    tx_metadata: &solana_transaction_status::TransactionStatusMeta,
    yu_mint: &str,
    min_amount: Option<f64>,
) -> bool {
    // Check pre-token balances
    if let Some(balances) = &tx_metadata.pre_token_balances {
        for balance in balances {
            if balance.mint == yu_mint {
                if let Some(min) = min_amount {
                    if let Some(amount) = balance.ui_token_amount.ui_amount {
                        return amount >= min;
                    }
                } else {
                    return true;
                }
            }
        }
    }
    
    // Check post-token balances
    if let Some(balances) = &tx_metadata.post_token_balances {
        for balance in balances {
            if balance.mint == yu_mint {
                if let Some(min) = min_amount {
                    if let Some(amount) = balance.ui_token_amount.ui_amount {
                        return amount >= min;
                    }
                } else {
                    return true;
                }
            }
        }
    }
    
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_yu_config_default() {
        let config = YuMonitorConfig::default();
        assert!(config.yu_token_only);
        assert_eq!(config.min_yu_amount, Some(100.0));
        assert_eq!(config.skip_after_no_yu_slots, 100);
    }
}