use anyhow::Result;
use solana_client::rpc_config::RpcBlockConfig;
use solana_transaction_status::{TransactionDetails, UiTransactionEncoding};
use std::sync::Arc;
use tracing::{debug, info};

use crate::rpc_client_with_failover::RpcClientWithFailover;

/// Quick filter check without full transaction extraction
pub struct QuickFilterCheck {
    rpc_client: Arc<RpcClientWithFailover>,
    // Add your filter addresses here
    monitored_programs: Vec<String>,
    monitored_addresses: Vec<String>,
}

impl QuickFilterCheck {
    pub fn new(rpc_url: String, monitored_programs: Vec<String>, monitored_addresses: Vec<String>) -> Self {
        let rpc_client = Arc::new(RpcClientWithFailover::new(rpc_url));
        
        Self {
            rpc_client,
            monitored_programs,
            monitored_addresses,
        }
    }

    /// Quickly check if a slot might contain relevant transactions
    pub async fn slot_might_match(&self, slot: u64) -> Result<bool> {
        // Get block with just signatures and account keys
        let config = RpcBlockConfig {
            encoding: Some(UiTransactionEncoding::Base64),
            transaction_details: Some(TransactionDetails::Accounts),
            rewards: Some(false),
            commitment: None,
            max_supported_transaction_version: Some(0),
        };
        
        match self.rpc_client.get_block_with_config(slot, config).await {
            Ok(block) => {
                if let Some(transactions) = block.transactions {
                    // Quick check: does any transaction involve our monitored addresses?
                    for tx in transactions {
                        if let Some(meta) = &tx.meta {
                            // Check account keys
                            if let Some(account_keys) = tx.transaction.message().account_keys() {
                                for key in account_keys {
                                    let key_str = key.to_string();
                                    if self.monitored_programs.contains(&key_str) || 
                                       self.monitored_addresses.contains(&key_str) {
                                        debug!("Slot {} might contain relevant transactions", slot);
                                        return Ok(true);
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(false)
            }
            Err(_) => Ok(false), // Slot doesn't exist
        }
    }
}