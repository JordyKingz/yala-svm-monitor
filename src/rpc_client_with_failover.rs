use anyhow::{Result, Context};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcBlockConfig, RpcSignatureStatusConfig};
use solana_client::rpc_response::RpcVersionInfo;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedConfirmedBlock, UiConfirmedBlock};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

#[derive(Clone)]
pub struct RpcClientWithFailover {
    rpc_urls: Vec<String>,
    current_index: Arc<RwLock<usize>>,
    max_retries: usize,
}

impl RpcClientWithFailover {
    pub fn new(primary_url: String) -> Self {
        let mut rpc_urls = vec![primary_url];
        
        // Add additional RPC URLs from environment variables
        if let Ok(url2) = std::env::var("SOLANA_RPC_URL_2") {
            if !url2.is_empty() {
                rpc_urls.push(url2);
            }
        }
        
        if let Ok(url3) = std::env::var("SOLANA_RPC_URL_3") {
            if !url3.is_empty() {
                rpc_urls.push(url3);
            }
        }

        if let Ok(url4) = std::env::var("SOLANA_RPC_URL_4") {
            if !url4.is_empty() {
                rpc_urls.push(url4);
            }
        }

        if let Ok(url5) = std::env::var("SOLANA_RPC_URL_5") {
            if !url5.is_empty() {
                rpc_urls.push(url5);
            }
        }
        
        info!("Initialized RPC client with {} URLs", rpc_urls.len());
        
        Self {
            rpc_urls,
            current_index: Arc::new(RwLock::new(0)),
            max_retries: 3,
        }
    }
    
    async fn get_current_client(&self) -> RpcClient {
        let index = *self.current_index.read().await;
        let url = &self.rpc_urls[index];
        RpcClient::new_with_timeout(url.clone(), Duration::from_secs(10))
    }
    
    async fn rotate_to_next_url(&self) -> Result<()> {
        let mut index = self.current_index.write().await;
        let next_index = (*index + 1) % self.rpc_urls.len();
        
        info!(
            "Rotating RPC URL from {} to {}", 
            self.rpc_urls[*index], 
            self.rpc_urls[next_index]
        );
        
        *index = next_index;
        Ok(())
    }
    
    async fn execute_with_failover<T, F>(&self, operation_name: &str, f: F) -> Result<T>
    where
        F: Fn(&RpcClient) -> Result<T>,
    {
        let mut last_error = None;
        let total_urls = self.rpc_urls.len();
        
        for attempt in 0..total_urls {
            let client = self.get_current_client().await;
            let current_url = self.rpc_urls[*self.current_index.read().await].clone();
            
            match f(&client) {
                Ok(result) => {
                    if attempt > 0 {
                        info!("Successfully completed {} after {} attempts", operation_name, attempt + 1);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    let error_str = e.to_string();
                    
                    // Check if this is a 429 error
                    if error_str.contains("429") || error_str.contains("Too Many Requests") {
                        warn!(
                            "RPC rate limit (429) encountered on {} for {}: {}", 
                            current_url, 
                            operation_name, 
                            error_str
                        );
                        
                        // Rotate to next URL
                        if attempt < total_urls - 1 {
                            self.rotate_to_next_url().await?;
                            continue;
                        }
                    } else {
                        // For non-429 errors, still try to rotate but log differently
                        error!(
                            "RPC error on {} for {}: {}", 
                            current_url, 
                            operation_name, 
                            error_str
                        );
                        
                        if attempt < total_urls - 1 {
                            self.rotate_to_next_url().await?;
                            continue;
                        }
                    }
                    
                    last_error = Some(e);
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All RPC URLs failed for {}", operation_name)))
    }
    
    pub async fn get_block_with_config(
        &self,
        slot: u64,
        config: RpcBlockConfig,
    ) -> Result<UiConfirmedBlock> {
        self.execute_with_failover("get_block_with_config", |client| {
            client.get_block_with_config(slot, config)
                .context(format!("Failed to get block for slot {}", slot))
        }).await
    }
    
    pub async fn get_slot(&self) -> Result<u64> {
        self.execute_with_failover("get_slot", |client| {
            client.get_slot()
                .context("Failed to get current slot")
        }).await
    }
    
    pub async fn get_account(&self, pubkey: &Pubkey) -> Result<solana_sdk::account::Account> {
        self.execute_with_failover("get_account", |client| {
            client.get_account(pubkey)
                .context(format!("Failed to get account {}", pubkey))
        }).await
    }
    
    pub async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
    ) -> Result<Vec<solana_client::rpc_response::RpcConfirmedTransactionStatusWithSignature>> {
        self.execute_with_failover("get_signatures_for_address", |client| {
            client.get_signatures_for_address(address)
                .context(format!("Failed to get signatures for address {}", address))
        }).await
    }
    
    pub async fn get_latest_blockhash(&self) -> Result<solana_sdk::hash::Hash> {
        self.execute_with_failover("get_latest_blockhash", |client| {
            client.get_latest_blockhash()
                .context("Failed to get latest blockhash")
        }).await
    }
    
    pub async fn get_slot_leaders(&self, start_slot: u64, limit: u64) -> Result<Vec<Pubkey>> {
        self.execute_with_failover("get_slot_leaders", |client| {
            client.get_slot_leaders(start_slot, limit)
                .context(format!("Failed to get slot leaders for slot {}", start_slot))
        }).await
    }
    
    pub async fn get_version(&self) -> Result<RpcVersionInfo> {
        self.execute_with_failover("get_version", |client| {
            client.get_version()
                .context("Failed to get version")
        }).await
    }
}