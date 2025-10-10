use anyhow::{Result, Context};
use futures::{stream::FuturesUnordered, StreamExt};
use std::sync::Arc;
use tokio::sync::{Semaphore, mpsc, RwLock};
use tracing::{info, error, debug};
use std::collections::BTreeMap;

use crate::filtered_monitor::{FilteredTransactionMonitor, StoredTransaction};

/// Results from processing a single slot
#[derive(Debug, Clone)]
pub struct SlotProcessingResult {
    pub slot: u64,
    pub matched_transactions: Vec<StoredTransaction>,
    pub success: bool,
    pub error: Option<String>,
}

/// Configuration for concurrent slot processing
#[derive(Debug, Clone)]
pub struct ConcurrentProcessorConfig {
    /// Maximum number of slots to process concurrently
    pub max_concurrent_slots: usize,
    /// Buffer size for the results channel
    pub channel_buffer_size: usize,
    /// Whether to maintain strict slot ordering in results
    pub maintain_order: bool,
}

impl Default for ConcurrentProcessorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_slots: 20,
            channel_buffer_size: 100,
            maintain_order: true,
        }
    }
}

/// Processes multiple slots concurrently while optionally maintaining order
pub struct ConcurrentSlotProcessor {
    monitor: Arc<FilteredTransactionMonitor>,
    config: ConcurrentProcessorConfig,
    semaphore: Arc<Semaphore>,
}

impl ConcurrentSlotProcessor {
    /// Create a new concurrent slot processor
    pub fn new(
        monitor: Arc<FilteredTransactionMonitor>,
        config: ConcurrentProcessorConfig,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_slots));
        
        Self {
            monitor,
            config,
            semaphore,
        }
    }

    /// Process a range of slots concurrently
    /// Returns a channel receiver that yields results as they complete
    pub async fn process_slot_range(
        &self,
        start_slot: u64,
        end_slot: u64,
    ) -> mpsc::Receiver<SlotProcessingResult> {
        let (tx, rx) = mpsc::channel(self.config.channel_buffer_size);
        
        let monitor = self.monitor.clone();
        let semaphore = self.semaphore.clone();
        let maintain_order = self.config.maintain_order;
        
        tokio::spawn(async move {
            if maintain_order {
                Self::process_with_ordering(monitor, semaphore, start_slot, end_slot, tx).await;
            } else {
                Self::process_unordered(monitor, semaphore, start_slot, end_slot, tx).await;
            }
        });
        
        rx
    }

    /// Process slots with maintained ordering
    async fn process_with_ordering(
        monitor: Arc<FilteredTransactionMonitor>,
        semaphore: Arc<Semaphore>,
        start_slot: u64,
        end_slot: u64,
        tx: mpsc::Sender<SlotProcessingResult>,
    ) {
        // Buffer to maintain order
        let buffer = Arc::new(RwLock::new(BTreeMap::<u64, SlotProcessingResult>::new()));
        let next_slot_to_send = Arc::new(RwLock::new(start_slot));
        
        let mut futures = FuturesUnordered::new();
        
        for slot in start_slot..=end_slot {
            let monitor = monitor.clone();
            let semaphore = semaphore.clone();
            let buffer = buffer.clone();
            let next_slot_to_send = next_slot_to_send.clone();
            let tx = tx.clone();
            
            let future = async move {
                // Acquire semaphore permit
                let _permit = semaphore.acquire().await.unwrap();
                
                debug!("Processing slot {} concurrently", slot);
                
                // Process the slot
                let result = match monitor.monitor_slot(slot).await {
                    Ok(matched_transactions) => SlotProcessingResult {
                        slot,
                        matched_transactions,
                        success: true,
                        error: None,
                    },
                    Err(e) => {
                        error!("Failed to process slot {}: {}", slot, e);
                        SlotProcessingResult {
                            slot,
                            matched_transactions: vec![],
                            success: false,
                            error: Some(e.to_string()),
                        }
                    }
                };
                
                // Store result in buffer
                {
                    let mut buf = buffer.write().await;
                    buf.insert(slot, result);
                }
                
                // Try to send any ready results in order
                {
                    let mut next_slot = next_slot_to_send.write().await;
                    let mut buf = buffer.write().await;
                    
                    while let Some(result) = buf.remove(&*next_slot) {
                        if tx.send(result).await.is_err() {
                            break; // Receiver dropped
                        }
                        *next_slot += 1;
                    }
                }
            };
            
            futures.push(future);
        }
        
        // Process all futures
        while let Some(_) = futures.next().await {
            // Futures handle their own result sending
        }
    }

    /// Process slots without maintaining order (faster)
    async fn process_unordered(
        monitor: Arc<FilteredTransactionMonitor>,
        semaphore: Arc<Semaphore>,
        start_slot: u64,
        end_slot: u64,
        tx: mpsc::Sender<SlotProcessingResult>,
    ) {
        let mut futures = FuturesUnordered::new();
        
        for slot in start_slot..=end_slot {
            let monitor = monitor.clone();
            let semaphore = semaphore.clone();
            let tx = tx.clone();
            
            let future = async move {
                // Acquire semaphore permit
                let _permit = semaphore.acquire().await.unwrap();
                
                debug!("Processing slot {} concurrently", slot);
                
                // Process the slot
                let result = match monitor.monitor_slot(slot).await {
                    Ok(matched_transactions) => SlotProcessingResult {
                        slot,
                        matched_transactions,
                        success: true,
                        error: None,
                    },
                    Err(e) => {
                        error!("Failed to process slot {}: {}", slot, e);
                        SlotProcessingResult {
                            slot,
                            matched_transactions: vec![],
                            success: false,
                            error: Some(e.to_string()),
                        }
                    }
                };
                
                // Send result immediately
                let _ = tx.send(result).await;
            };
            
            futures.push(future);
        }
        
        // Process all futures
        while let Some(_) = futures.next().await {
            // Futures handle their own result sending
        }
    }

    /// Process a batch of specific slots (not necessarily sequential)
    pub async fn process_slots(
        &self,
        slots: Vec<u64>,
    ) -> mpsc::Receiver<SlotProcessingResult> {
        let (tx, rx) = mpsc::channel(self.config.channel_buffer_size);
        
        let monitor = self.monitor.clone();
        let semaphore = self.semaphore.clone();
        
        tokio::spawn(async move {
            let mut futures = FuturesUnordered::new();
            
            for slot in slots {
                let monitor = monitor.clone();
                let semaphore = semaphore.clone();
                let tx = tx.clone();
                
                let future = async move {
                    // Acquire semaphore permit
                    let _permit = semaphore.acquire().await.unwrap();
                    
                    debug!("Processing slot {} concurrently", slot);
                    
                    // Process the slot
                    let result = match monitor.monitor_slot(slot).await {
                        Ok(matched_transactions) => SlotProcessingResult {
                            slot,
                            matched_transactions,
                            success: true,
                            error: None,
                        },
                        Err(e) => {
                            error!("Failed to process slot {}: {}", slot, e);
                            SlotProcessingResult {
                                slot,
                                matched_transactions: vec![],
                                success: false,
                                error: Some(e.to_string()),
                            }
                        }
                    };
                    
                    // Send result
                    let _ = tx.send(result).await;
                };
                
                futures.push(future);
            }
            
            // Process all futures
            while let Some(_) = futures.next().await {
                // Futures handle their own result sending
            }
        });
        
        rx
    }
}

/// Builder pattern for ConcurrentSlotProcessor
pub struct ConcurrentProcessorBuilder {
    config: ConcurrentProcessorConfig,
}

impl ConcurrentProcessorBuilder {
    pub fn new() -> Self {
        Self {
            config: ConcurrentProcessorConfig::default(),
        }
    }
    
    pub fn max_concurrent_slots(mut self, max: usize) -> Self {
        self.config.max_concurrent_slots = max;
        self
    }
    
    pub fn channel_buffer_size(mut self, size: usize) -> Self {
        self.config.channel_buffer_size = size;
        self
    }
    
    pub fn maintain_order(mut self, maintain: bool) -> Self {
        self.config.maintain_order = maintain;
        self
    }
    
    pub fn build(self, monitor: Arc<FilteredTransactionMonitor>) -> ConcurrentSlotProcessor {
        ConcurrentSlotProcessor::new(monitor, self.config)
    }
}