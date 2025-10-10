use anyhow::{Result, Context};
use futures::stream::{FuturesUnordered, StreamExt};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tracing::{info, error, debug, warn};
use std::time::Instant;
use std::collections::HashMap;

use crate::filtered_monitor::{FilteredTransactionMonitor, StoredTransaction};

#[derive(Debug, Clone)]
pub struct SlotProcessingResult {
    pub slot: u64,
    pub matched_transactions: Vec<StoredTransaction>,
    pub success: bool,
    pub error: Option<String>,
    pub processing_time_ms: u64,
}

pub struct ConcurrentSlotProcessor {
    monitor: Arc<FilteredTransactionMonitor>,
    max_concurrent_slots: usize,
}

impl ConcurrentSlotProcessor {
    pub fn new(
        monitor: Arc<FilteredTransactionMonitor>,
        _rpc_url: String,
        max_concurrent_slots: Option<usize>,
    ) -> Self {
        let max_concurrent = max_concurrent_slots.unwrap_or(20);
        info!("Initialized concurrent processor with max {} concurrent slots", max_concurrent);
        
        Self {
            monitor,
            max_concurrent_slots: max_concurrent,
        }
    }

    /// Process multiple slots concurrently
    pub async fn process_slots(
        &self,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<Vec<SlotProcessingResult>> {
        let total_slots = end_slot - start_slot + 1;
        info!("🚀 Starting concurrent processing of {} slots ({}..{})", 
            total_slots, start_slot, end_slot);
        
        let start_time = Instant::now();
        let semaphore = Arc::new(Semaphore::new(self.max_concurrent_slots));
        let (tx, mut rx) = mpsc::channel::<SlotProcessingResult>(100);
        
        // Create a pool of futures for processing slots
        let mut futures = FuturesUnordered::new();
        
        // Statistics tracking
        let mut slot_times = HashMap::new();
        
        for (idx, slot) in (start_slot..=end_slot).enumerate() {
            let semaphore = semaphore.clone();
            let tx = tx.clone();
            
            // Round-robin fast monitor selection for better load distribution
            let monitor = self.monitor.clone();
            
            futures.push(async move {
                let slot_start = Instant::now();
                let _permit = semaphore.acquire().await.unwrap();
                
                debug!("Processing slot {}", slot);
                
                let result = match monitor.monitor_slot(slot).await {
                    Ok(matched_transactions) => {
                        let processing_time = slot_start.elapsed().as_millis() as u64;
                        if !matched_transactions.is_empty() {
                            info!("✅ Slot {} found {} matches in {}ms", 
                                slot, matched_transactions.len(), processing_time);
                        }
                        SlotProcessingResult {
                            slot,
                            matched_transactions,
                            success: true,
                            error: None,
                            processing_time_ms: processing_time,
                        }
                    }
                    Err(e) => {
                        let processing_time = slot_start.elapsed().as_millis() as u64;
                        warn!("❌ Slot {} failed after {}ms: {}", slot, processing_time, e);
                        SlotProcessingResult {
                            slot,
                            matched_transactions: vec![],
                            success: false,
                            error: Some(e.to_string()),
                            processing_time_ms: processing_time,
                        }
                    }
                };
                
                if let Err(e) = tx.send(result.clone()).await {
                    error!("Failed to send result for slot {}: {}", slot, e);
                }
                
                (slot, result.processing_time_ms)
            });
        }
        
        // Drop the original sender to signal completion
        drop(tx);
        
        // Collect results and statistics
        let mut results = Vec::new();
        let mut processed_count = 0;
        let mut success_count = 0;
        let mut total_matches = 0;
        
        // Process futures and collect timing stats
        let processing_handle = tokio::spawn(async move {
            while let Some((slot, time_ms)) = futures.next().await {
                slot_times.insert(slot, time_ms);
            }
            slot_times
        });
        
        // Collect results
        while let Some(result) = rx.recv().await {
            if result.success {
                success_count += 1;
                total_matches += result.matched_transactions.len();
            }
            processed_count += 1;
            
            // Progress update every 100 slots
            if processed_count % 100 == 0 {
                let elapsed = start_time.elapsed();
                let rate = processed_count as f64 / elapsed.as_secs_f64();
                info!("📊 Progress: {}/{} slots ({:.1} slots/sec)", 
                    processed_count, total_slots, rate);
            }
            
            results.push(result);
        }
        
        // Get timing statistics
        let slot_times = processing_handle.await.unwrap();
        
        // Calculate statistics
        let total_duration = start_time.elapsed();
        let avg_rate = total_slots as f64 / total_duration.as_secs_f64();
        
        // Calculate timing percentiles
        let mut times: Vec<u64> = slot_times.values().copied().collect();
        times.sort_unstable();
        
        let p50 = times.get(times.len() / 2).copied().unwrap_or(0);
        let p95 = times.get(times.len() * 95 / 100).copied().unwrap_or(0);
        let p99 = times.get(times.len() * 99 / 100).copied().unwrap_or(0);
        
        info!("✅ Concurrent processing completed:");
        info!("   Total slots: {}", total_slots);
        info!("   Successful: {} ({:.1}%)", success_count, 
            success_count as f64 / total_slots as f64 * 100.0);
        info!("   Total matches: {}", total_matches);
        info!("   Total time: {:.2}s", total_duration.as_secs_f64());
        info!("   Average rate: {:.1} slots/sec", avg_rate);
        info!("   Slot processing times - P50: {}ms, P95: {}ms, P99: {}ms", p50, p95, p99);
        
        // Sort results by slot number
        results.sort_by_key(|r| r.slot);
        
        Ok(results)
    }
}