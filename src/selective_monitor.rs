use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{info, debug};
use tokio::sync::RwLock;
use chrono::Timelike;

use crate::slot_pre_filter::{SlotPreFilter, PreFilterConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectiveMonitorConfig {
    /// Minimum token amounts to consider (token mint -> minimum amount)
    pub minimum_amounts: HashMap<String, f64>,
    
    /// Programs that must be involved for monitoring
    pub required_programs: Vec<String>,
    
    /// Time-based filtering (skip monitoring during low activity hours)
    pub active_hours: Option<(u8, u8)>, // (start_hour, end_hour) in UTC
    
    /// Skip slots that have been empty for N consecutive slots
    pub skip_after_empty_slots: Option<u32>,
    
    /// Only monitor if slot has minimum number of transactions
    pub min_transactions_per_slot: Option<usize>,
    
    /// Dynamic adjustment based on activity
    pub dynamic_filtering: bool,
}

impl Default for SelectiveMonitorConfig {
    fn default() -> Self {
        Self {
            minimum_amounts: HashMap::new(),
            required_programs: vec![],
            active_hours: None,
            skip_after_empty_slots: Some(10),
            min_transactions_per_slot: None,
            dynamic_filtering: true,
        }
    }
}

pub struct SelectiveMonitor {
    config: SelectiveMonitorConfig,
    pre_filter: Arc<SlotPreFilter>,
    
    // Track activity patterns
    activity_tracker: Arc<RwLock<ActivityTracker>>,
    
    // Cache of recently seen token activities
    token_activity_cache: Arc<RwLock<HashMap<String, TokenActivity>>>,
}

#[derive(Debug, Default)]
struct ActivityTracker {
    consecutive_empty_slots: u32,
    last_activity_slot: u64,
    hourly_activity: [u32; 24], // Activity count per hour
    token_last_seen: HashMap<String, u64>, // Token -> last slot seen
}

#[derive(Debug, Clone)]
struct TokenActivity {
    last_seen_slot: u64,
    recent_volume: f64,
    transaction_count: u32,
}

impl SelectiveMonitor {
    pub fn new(
        rpc_url: String,
        config: SelectiveMonitorConfig,
        pre_filter_config: PreFilterConfig,
    ) -> Self {
        let pre_filter = Arc::new(SlotPreFilter::new(rpc_url, pre_filter_config));
        
        Self {
            config,
            pre_filter,
            activity_tracker: Arc::new(RwLock::new(ActivityTracker::default())),
            token_activity_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Load selective monitor config from monitor configurations
    pub fn from_monitor_configs(
        rpc_url: String,
        monitor_configs: &[serde_json::Value],
    ) -> Result<Self> {
        let mut minimum_amounts = HashMap::new();
        let mut required_programs = HashSet::new();
        let mut monitored_tokens = HashSet::new();
        
        // Extract minimum amounts and programs from monitor conditions
        for monitor in monitor_configs {
            if let Some(conditions) = monitor.get("conditions") {
                // Check all_of conditions
                if let Some(all_of) = conditions.get("all_of").and_then(|v| v.as_array()) {
                    for condition in all_of {
                        // Extract token transfer conditions
                        if condition.get("type").and_then(|v| v.as_str()) == Some("TokenTransfer") {
                            if let (Some(mint), Some(amount)) = (
                                condition.get("mint").and_then(|v| v.as_str()),
                                condition.get("amount").and_then(|v| v.as_f64())
                            ) {
                                monitored_tokens.insert(mint.to_string());
                                minimum_amounts.entry(mint.to_string())
                                    .and_modify(|e: &mut f64| *e = e.min(amount))
                                    .or_insert(amount);
                            }
                        }
                        
                        // Extract program conditions
                        if condition.get("type").and_then(|v| v.as_str()) == Some("ProgramInvoked") {
                            if let Some(program_id) = condition.get("program_id").and_then(|v| v.as_str()) {
                                required_programs.insert(program_id.to_string());
                            }
                        }
                    }
                }
                
                // Also check any_of conditions
                if let Some(any_of) = conditions.get("any_of").and_then(|v| v.as_array()) {
                    for condition in any_of {
                        // Extract token transfer conditions
                        if condition.get("type").and_then(|v| v.as_str()) == Some("TokenTransfer") {
                            if let Some(mint) = condition.get("mint").and_then(|v| v.as_str()) {
                                monitored_tokens.insert(mint.to_string());
                            }
                        }
                        
                        // Extract program conditions
                        if condition.get("type").and_then(|v| v.as_str()) == Some("ProgramInvoked") {
                            if let Some(program_id) = condition.get("program_id").and_then(|v| v.as_str()) {
                                required_programs.insert(program_id.to_string());
                            }
                        }
                    }
                }
            }
        }
        
        // Always include the Token Program for token transfers
        // required_programs.insert("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string());
        
        // Build config
        let config = SelectiveMonitorConfig {
            minimum_amounts,
            required_programs: required_programs.into_iter().collect(),
            active_hours: None, // Can be configured separately
            skip_after_empty_slots: Some(10),
            min_transactions_per_slot: Some(100), // Skip nearly empty slots
            dynamic_filtering: true,
        };
        
        // Build pre-filter config
        let monitored_tokens_vec: Vec<String> = monitored_tokens.iter().cloned().collect();
        let pre_filter_config = PreFilterConfig {
            monitored_programs: config.required_programs.clone(),
            monitored_tokens: monitored_tokens_vec.clone(),
        };
        
        info!(
            "Initialized selective monitor with {} minimum amounts, {} required programs",
            config.minimum_amounts.len(),
            config.required_programs.len()
        );
        
        // Log the monitored tokens for debugging
        info!("Monitored tokens: {:?}", monitored_tokens_vec);
        info!("Monitored programs: {:?}", config.required_programs);
        
        Ok(Self::new(rpc_url, config, pre_filter_config))
    }
    
    /// Determine if a slot range should be monitored based on selective criteria
    pub async fn should_monitor_slots(&self, slots: &[u64]) -> Result<Vec<u64>> {
        let mut slots_to_monitor = Vec::new();
        
        // First, use pre-filter to find potentially relevant slots
        let relevant_slots = self.pre_filter.filter_relevant_slots(slots.to_vec()).await?;
        
        if relevant_slots.is_empty() {
            // Update empty slot counter
            let mut tracker = self.activity_tracker.write().await;
            tracker.consecutive_empty_slots += slots.len() as u32;
            
            // If too many empty slots, start skipping larger ranges
            if let Some(skip_threshold) = self.config.skip_after_empty_slots {
                if tracker.consecutive_empty_slots > skip_threshold {
                    info!(
                        "Skipping monitoring after {} consecutive empty slots",
                        tracker.consecutive_empty_slots
                    );
                    return Ok(vec![]);
                }
            }
        } else {
            // Reset empty slot counter
            let mut tracker = self.activity_tracker.write().await;
            tracker.consecutive_empty_slots = 0;
        }
        
        // Apply time-based filtering
        if let Some((start_hour, end_hour)) = self.config.active_hours {
            let current_hour = chrono::Utc::now().hour() as u8;
            let is_active = if start_hour <= end_hour {
                current_hour >= start_hour && current_hour <= end_hour
            } else {
                // Handle wrap around midnight
                current_hour >= start_hour || current_hour <= end_hour
            };
            
            if !is_active {
                debug!("Outside active hours ({}-{}), skipping slots", start_hour, end_hour);
                return Ok(vec![]);
            }
        }
        
        // Check recent activity for dynamic filtering
        if self.config.dynamic_filtering {
            let activity = self.activity_tracker.read().await;
            
            // Check if any monitored token has been active recently
            let current_slot = slots.last().copied().unwrap_or(0);
            let mut has_recent_activity = false;
            
            for (token, last_seen) in &activity.token_last_seen {
                // If token was seen in last 1000 slots (~400 seconds)
                if current_slot.saturating_sub(*last_seen) < 1000 {
                    has_recent_activity = true;
                    break;
                }
            }
            
            if !has_recent_activity && activity.consecutive_empty_slots > 5 {
                // Reduce monitoring frequency when no recent activity
                // Only monitor every 10th slot
                slots_to_monitor = relevant_slots.into_iter()
                    .enumerate()
                    .filter_map(|(i, slot)| if i % 10 == 0 { Some(slot) } else { None })
                    .collect();
                
                debug!(
                    "Low activity detected, monitoring {} out of {} slots",
                    slots_to_monitor.len(),
                    slots.len()
                );
                
                return Ok(slots_to_monitor);
            }
        }
        
        Ok(relevant_slots)
    }
    
    /// Update activity tracking based on processed slot results
    pub async fn update_activity(
        &self,
        slot: u64,
        token_activities: Vec<(String, f64)>, // (token_mint, volume)
    ) -> Result<()> {
        let mut tracker = self.activity_tracker.write().await;
        let mut cache = self.token_activity_cache.write().await;
        
        if !token_activities.is_empty() {
            tracker.last_activity_slot = slot;
            tracker.consecutive_empty_slots = 0;
            
            // Update hourly activity
            let hour = chrono::Utc::now().hour() as usize;
            tracker.hourly_activity[hour] += 1;
            
            // Update token-specific activity
            for (token, volume) in token_activities {
                tracker.token_last_seen.insert(token.clone(), slot);
                
                // Update cache
                cache.entry(token.clone())
                    .and_modify(|activity| {
                        activity.last_seen_slot = slot;
                        activity.recent_volume += volume;
                        activity.transaction_count += 1;
                    })
                    .or_insert(TokenActivity {
                        last_seen_slot: slot,
                        recent_volume: volume,
                        transaction_count: 1,
                    });
            }
        }
        
        Ok(())
    }
    
    /// Get current activity statistics
    pub async fn get_activity_stats(&self) -> Result<ActivityStats> {
        let tracker = self.activity_tracker.read().await;
        let cache = self.token_activity_cache.read().await;
        
        let total_hourly_activity: u32 = tracker.hourly_activity.iter().sum();
        let active_hours = tracker.hourly_activity.iter()
            .filter(|&&count| count > 0)
            .count();
        
        let most_active_token = cache.iter()
            .max_by_key(|(_, activity)| activity.transaction_count)
            .map(|(token, _)| token.clone());
        
        Ok(ActivityStats {
            consecutive_empty_slots: tracker.consecutive_empty_slots,
            last_activity_slot: tracker.last_activity_slot,
            total_hourly_activity,
            active_hours,
            most_active_token,
            token_count: cache.len(),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ActivityStats {
    pub consecutive_empty_slots: u32,
    pub last_activity_slot: u64,
    pub total_hourly_activity: u32,
    pub active_hours: usize,
    pub most_active_token: Option<String>,
    pub token_count: usize,
}

/// Helper to build selective monitoring config from filters
pub fn build_selective_config(monitor_configs: &[serde_json::Value]) -> SelectiveMonitorConfig {
    let mut config = SelectiveMonitorConfig::default();
    
    // Extract all token minimum amounts
    for monitor in monitor_configs {
        if let Some(conditions) = monitor.get("conditions") {
            if let Some(all_of) = conditions.get("all_of").and_then(|v| v.as_array()) {
                for condition in all_of {
                    if condition.get("type").and_then(|v| v.as_str()) == Some("TokenTransfer") {
                        if let (Some(mint), Some(amount), Some(operator)) = (
                            condition.get("mint").and_then(|v| v.as_str()),
                            condition.get("amount").and_then(|v| v.as_f64()),
                            condition.get("operator").and_then(|v| v.as_str())
                        ) {
                            // Only track minimum amounts for GreaterThanOrEqual conditions
                            if operator == "GreaterThanOrEqual" {
                                config.minimum_amounts.entry(mint.to_string())
                                    .and_modify(|e: &mut f64| *e = e.min(amount))
                                    .or_insert(amount);
                            }
                        }
                    }
                }
            }
        }
    }
    
    info!(
        "Built selective config with {} token minimum amounts",
        config.minimum_amounts.len()
    );
    
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_selective_config_from_monitors() {
        let monitor_json = r#"[{
            "id": "test",
            "conditions": {
                "all_of": [{
                    "type": "TokenTransfer",
                    "mint": "YUYAiJo8KVbnc6Fb6h3MnH2VGND4uGWDH4iLnw7DLEu",
                    "operator": "GreaterThanOrEqual",
                    "amount": 1000000.0
                }]
            }
        }]"#;
        
        let monitors: Vec<serde_json::Value> = serde_json::from_str(monitor_json).unwrap();
        let config = build_selective_config(&monitors);
        
        assert_eq!(config.minimum_amounts.len(), 1);
        assert_eq!(
            config.minimum_amounts.get("YUYAiJo8KVbnc6Fb6h3MnH2VGND4uGWDH4iLnw7DLEu"),
            Some(&1000000.0)
        );
    }
}