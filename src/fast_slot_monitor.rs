use anyhow::{Result, Context};
use solana_client::rpc_config::RpcBlockConfig;
use solana_transaction_status::TransactionDetails;
use std::sync::Arc;
use tracing::{info, debug};

use crate::filtered_monitor::{FilteredTransactionMonitor, StoredTransaction};
use crate::transaction_extractor::TransactionExtractor;
use crate::parallel_filter_processor::ParallelFilterProcessor;
use crate::rpc_client_with_failover::RpcClientWithFailover;

/// Fast slot monitoring that skips unnecessary processing
pub struct FastSlotMonitor {
    rpc_client: Arc<RpcClientWithFailover>,
    transaction_extractor: Arc<TransactionExtractor>,
    filter_processor: Arc<ParallelFilterProcessor>,
    monitor: Arc<FilteredTransactionMonitor>,
}

impl FastSlotMonitor {
    pub fn new(
        rpc_url: String,
        monitor: Arc<FilteredTransactionMonitor>,
    ) -> Self {
        let rpc_client = Arc::new(RpcClientWithFailover::new(rpc_url.clone()));
        let transaction_extractor = Arc::new(TransactionExtractor::new(rpc_url));
        let filter_processor = Arc::new(ParallelFilterProcessor::new(monitor.filter_engine.clone()));
        
        Self {
            rpc_client,
            transaction_extractor,
            filter_processor,
            monitor,
        }
    }

    /// Fast check if a slot might contain matching transactions
    pub async fn quick_check_slot(&self, slot: u64) -> Result<bool> {
        // First just get the slot metadata without transaction details
        let config = RpcBlockConfig {
            encoding: Some(solana_transaction_status::UiTransactionEncoding::Base64),
            transaction_details: Some(TransactionDetails::None),
            rewards: Some(false),
            commitment: None,
            max_supported_transaction_version: Some(0),
        };
        
        match self.rpc_client.get_block_with_config(slot, config).await {
            Ok(block) => {
                let tx_count = block.transactions.map(|txs| txs.len()).unwrap_or(0);
                debug!("Slot {} has {} transactions", slot, tx_count);
                Ok(tx_count > 0)
            }
            Err(_) => {
                // Slot doesn't exist or is empty
                Ok(false)
            }
        }
    }

    /// Process a slot with optimizations
    pub async fn process_slot(&self, slot: u64) -> Result<Vec<StoredTransaction>> {
        // Quick check first
        if !self.quick_check_slot(slot).await? {
            debug!("Slot {} is empty, skipping", slot);
            return Ok(vec![]);
        }
        
        // Extract transactions
        let transactions = self.transaction_extractor.extract_from_slot(slot).await?;
        
        if transactions.is_empty() {
            return Ok(vec![]);
        }
        
        info!("Processing {} transactions from slot {} in parallel", transactions.len(), slot);
        
        // Process transactions in parallel
        let matched = self.filter_processor.process_transactions(transactions).await;
        
        // Convert to StoredTransaction format
        let stored: Vec<StoredTransaction> = matched.into_iter()
            .map(|(tx, filters)| StoredTransaction {
                transaction: tx,
                matched_filters: filters.iter().map(|f| f.filter_id.clone()).collect(),
                stored_at: chrono::Utc::now(),
                collection: "default".to_string(),
            })
            .collect();
        
        if !stored.is_empty() {
            info!("Slot {} matched {} transactions", slot, stored.len());
        }
        
        Ok(stored)
    }
}