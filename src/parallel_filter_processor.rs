use anyhow::Result;
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use tracing::{debug, info};

use crate::filter_engine::{FilterEngine, MatchedFilter};
use crate::transaction_extractor::ExtractedTransaction;

/// Process transactions through filters in parallel
pub struct ParallelFilterProcessor {
    filter_engine: Arc<FilterEngine>,
    chunk_size: usize,
}

impl ParallelFilterProcessor {
    pub fn new(filter_engine: Arc<FilterEngine>) -> Self {
        Self {
            filter_engine,
            chunk_size: 100, // Process transactions in chunks of 100
        }
    }

    /// Process multiple transactions in parallel
    pub async fn process_transactions(
        &self,
        transactions: Vec<ExtractedTransaction>,
    ) -> Vec<(ExtractedTransaction, Vec<MatchedFilter>)> {
        let total_txs = transactions.len();
        debug!("Processing {} transactions in parallel", total_txs);
        
        // Process transactions in parallel chunks
        let results: Vec<(ExtractedTransaction, Vec<MatchedFilter>)> = stream::iter(transactions)
            .chunks(self.chunk_size)
            .map(|chunk| {
                let filter_engine = self.filter_engine.clone();
                async move {
                    // Process chunk of transactions
                    let mut chunk_results = Vec::new();
                    for tx in chunk {
                        let matched_filters = filter_engine.evaluate_transaction(&tx);
                        if !matched_filters.is_empty() {
                            chunk_results.push((tx, matched_filters));
                        }
                    }
                    chunk_results
                }
            })
            .buffer_unordered(10) // Process up to 10 chunks concurrently
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .collect();
        
        debug!("Found {} matching transactions out of {}", results.len(), total_txs);
        results
    }
}