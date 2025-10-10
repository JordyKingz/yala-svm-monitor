use anyhow::{Result, Context};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use std::time::{Duration, Instant};
use std::collections::HashMap;

use futures::future;
use chrono;
use uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionEvent {
    pub signature: String,
    pub slot: u64,
    pub success: bool,
    pub fee: u64,
}

#[async_trait]
pub trait NotificationChannel: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<()>;
    fn name(&self) -> &str;
    fn is_enabled(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub timestamp: i64,
    pub severity: AlertSeverity,
    pub title: String,
    pub message: String,
    pub transaction: Option<TransactionEvent>,
    pub metadata: HashMap<String, String>,
}

pub struct NotificationManager {
    channels: Arc<RwLock<Vec<Box<dyn NotificationChannel>>>>,
    rate_limiter: Arc<RwLock<RateLimiter>>,
    deduplication_cache: Arc<RwLock<DeduplicationCache>>,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(Vec::new())),
            rate_limiter: Arc::new(RwLock::new(RateLimiter::new())),
            deduplication_cache: Arc::new(RwLock::new(DeduplicationCache::new())),
        }
    }

    pub async fn add_channel(&self, channel: Box<dyn NotificationChannel>) {
        let mut channels = self.channels.write().await;
        info!("Adding notification channel: {}", channel.name());
        channels.push(channel);
    }
    
    pub fn add_notification(&mut self, title: &str, message: &str, level: NotificationLevel) {
        let severity = match level {
            NotificationLevel::Info => AlertSeverity::Low,
            NotificationLevel::Warning => AlertSeverity::Medium,
            NotificationLevel::Error => AlertSeverity::High,
            NotificationLevel::Critical => AlertSeverity::Critical,
        };
        
        let alert = Alert {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            severity,
            title: title.to_string(),
            message: message.to_string(),
            transaction: None,
            metadata: HashMap::new(),
        };
        
        // Store alert for later sending
        // In a real implementation, you might queue this for async sending
        info!("Notification added: {} - {}", title, message);
    }

    pub async fn send_alert(&self, alert: Alert) -> Result<()> {
        // Check for duplicate alerts
        let mut dedup_cache = self.deduplication_cache.write().await;
        if dedup_cache.is_duplicate(&alert) {
            info!("Skipping duplicate alert: {}", alert.id);
            return Ok(());
        }
        dedup_cache.add(&alert);

        // Check rate limits
        let mut rate_limiter = self.rate_limiter.write().await;
        if !rate_limiter.check_limit(&alert.severity) {
            warn!("Rate limit exceeded for severity {:?}", alert.severity);
            return Ok(());
        }

        // Send to all enabled channels
        let channels = self.channels.read().await;
        let mut send_futures = vec![];
        
        for channel in channels.iter() {
            if channel.is_enabled() {
                let alert_clone = alert.clone();
                let channel_name = channel.name().to_string();
                
                // Clone the channel reference for async move
                let channel_clone = channel.as_ref();
                
                send_futures.push(async move {
                    match channel_clone.send(&alert_clone).await {
                        Ok(_) => info!("Alert sent successfully via {}", channel_name),
                        Err(e) => error!("Failed to send alert via {}: {}", channel_name, e),
                    }
                });
            }
        }

        // Execute all sends concurrently
        futures::future::join_all(send_futures).await;

        Ok(())
    }
}

// Slack notification implementation
pub struct SlackChannel {
    webhook_url: String,
    client: Client,
    enabled: bool,
}

impl SlackChannel {
    pub fn new(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: Client::new(),
            enabled: true,
        }
    }

    fn format_slack_message(&self, alert: &Alert) -> serde_json::Value {
        let color = match alert.severity {
            AlertSeverity::Low => "#36a64f",      // Green
            AlertSeverity::Medium => "#ff9800",    // Orange
            AlertSeverity::High => "#ff5722",      // Red
            AlertSeverity::Critical => "#f44336",   // Dark Red
        };

        let mut fields = vec![
            json!({
                "title": "Severity",
                "value": format!("{:?}", alert.severity),
                "short": true
            }),
            json!({
                "title": "Timestamp",
                "value": format!("<t:{}:F>", alert.timestamp),
                "short": true
            }),
        ];

        if let Some(tx) = &alert.transaction {
            fields.push(json!({
                "title": "Transaction",
                "value": format!("<https://solscan.io/tx/{}|{}>", tx.signature, &tx.signature[..8]),
                "short": false
            }));
            fields.push(json!({
                "title": "Slot",
                "value": tx.slot.to_string(),
                "short": true
            }));
            fields.push(json!({
                "title": "Fee",
                "value": format!("{} lamports", tx.fee),
                "short": true
            }));
        }

        json!({
            "attachments": [{
                "fallback": alert.message.clone(),
                "color": color,
                "title": alert.title.clone(),
                "text": alert.message.clone(),
                "fields": fields,
                "footer": "Solana Monitor",
                "ts": alert.timestamp
            }]
        })
    }
}

#[async_trait]
impl NotificationChannel for SlackChannel {
    async fn send(&self, alert: &Alert) -> Result<()> {
        let payload = self.format_slack_message(alert);
        
        let response = self.client
            .post(&self.webhook_url)
            .json(&payload)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .context("Failed to send Slack notification")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Slack webhook failed with status {}: {}", status, body);
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "Slack"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// Discord notification implementation
pub struct DiscordChannel {
    webhook_url: String,
    client: Client,
    enabled: bool,
}

impl DiscordChannel {
    pub fn new(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: Client::new(),
            enabled: true,
        }
    }

    fn format_discord_message(&self, alert: &Alert) -> serde_json::Value {
        let color = match alert.severity {
            AlertSeverity::Low => 0x36a64f,      // Green
            AlertSeverity::Medium => 0xff9800,    // Orange
            AlertSeverity::High => 0xff5722,      // Red
            AlertSeverity::Critical => 0xf44336,   // Dark Red
        };

        let mut embeds = vec![json!({
            "title": alert.title.clone(),
            "description": alert.message.clone(),
            "color": color,
            "timestamp": chrono::DateTime::from_timestamp(alert.timestamp, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            "fields": []
        })];

        if let Some(embed) = embeds.get_mut(0) {
            if let Some(fields) = embed.get_mut("fields").and_then(|f| f.as_array_mut()) {
                fields.push(json!({
                    "name": "Severity",
                    "value": format!("{:?}", alert.severity),
                    "inline": true
                }));

                if let Some(tx) = &alert.transaction {
                    fields.push(json!({
                        "name": "Transaction",
                        "value": format!("[View on Solscan](https://solscan.io/tx/{})", tx.signature),
                        "inline": false
                    }));
                    fields.push(json!({
                        "name": "Slot",
                        "value": tx.slot.to_string(),
                        "inline": true
                    }));
                    fields.push(json!({
                        "name": "Success",
                        "value": if tx.success { "✅ Success" } else { "❌ Failed" },
                        "inline": true
                    }));
                }
            }
        }

        json!({
            "embeds": embeds
        })
    }
}

#[async_trait]
impl NotificationChannel for DiscordChannel {
    async fn send(&self, alert: &Alert) -> Result<()> {
        let payload = self.format_discord_message(alert);
        
        let response = self.client
            .post(&self.webhook_url)
            .json(&payload)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .context("Failed to send Discord notification")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Discord webhook failed with status {}: {}", status, body);
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "Discord"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// Console notification implementation (for debugging)
pub struct ConsoleChannel {
    enabled: bool,
}

impl ConsoleChannel {
    pub fn new() -> Self {
        Self { enabled: true }
    }
}

#[async_trait]
impl NotificationChannel for ConsoleChannel {
    async fn send(&self, alert: &Alert) -> Result<()> {
        let severity_emoji = match alert.severity {
            AlertSeverity::Low => "ℹ️",
            AlertSeverity::Medium => "⚠️",
            AlertSeverity::High => "🚨",
            AlertSeverity::Critical => "🔴",
        };

        println!("\n{} {} ALERT: {}", severity_emoji, alert.severity.to_string(), alert.title);
        println!("Message: {}", alert.message);
        
        if let Some(tx) = &alert.transaction {
            println!("Transaction: https://solscan.io/tx/{}", tx.signature);
            println!("Slot: {} | Fee: {} lamports | Success: {}", 
                tx.slot, tx.fee, tx.success);
        }

        println!("Timestamp: {}", chrono::DateTime::from_timestamp(alert.timestamp, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default());
        println!("{}", "-".repeat(80));

        Ok(())
    }

    fn name(&self) -> &str {
        "Console"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// Rate limiter to prevent notification spam
struct RateLimiter {
    limits: HashMap<String, RateLimit>,
}

struct RateLimit {
    max_per_minute: usize,
    current_count: usize,
    window_start: Instant,
}

impl RateLimiter {
    fn new() -> Self {
        let mut limits = HashMap::new();
        
        // Configure rate limits per severity
        limits.insert("Low".to_string(), RateLimit {
            max_per_minute: 60,
            current_count: 0,
            window_start: Instant::now(),
        });
        limits.insert("Medium".to_string(), RateLimit {
            max_per_minute: 30,
            current_count: 0,
            window_start: Instant::now(),
        });
        limits.insert("High".to_string(), RateLimit {
            max_per_minute: 20,
            current_count: 0,
            window_start: Instant::now(),
        });
        limits.insert("Critical".to_string(), RateLimit {
            max_per_minute: 10,
            current_count: 0,
            window_start: Instant::now(),
        });

        Self { limits }
    }

    fn check_limit(&mut self, severity: &AlertSeverity) -> bool {
        let severity_str = severity.to_string();
        
        if let Some(limit) = self.limits.get_mut(&severity_str) {
            // Reset window if minute has passed
            if limit.window_start.elapsed() > Duration::from_secs(60) {
                limit.current_count = 0;
                limit.window_start = Instant::now();
            }

            if limit.current_count < limit.max_per_minute {
                limit.current_count += 1;
                true
            } else {
                false
            }
        } else {
            true // Allow if no limit configured
        }
    }
}

// Deduplication cache to prevent duplicate alerts
struct DeduplicationCache {
    cache: HashMap<String, Instant>,
    ttl: Duration,
}

impl DeduplicationCache {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
            ttl: Duration::from_secs(300), // 5 minutes
        }
    }

    fn is_duplicate(&mut self, alert: &Alert) -> bool {
        // Clean expired entries
        self.cache.retain(|_, timestamp| timestamp.elapsed() < self.ttl);

        // Create cache key from alert content
        let cache_key = format!("{}-{}-{}", alert.title, alert.severity.to_string(), 
            alert.transaction.as_ref().map(|tx| &tx.signature).unwrap_or(&alert.id));

        if self.cache.contains_key(&cache_key) {
            true
        } else {
            false
        }
    }

    fn add(&mut self, alert: &Alert) {
        let cache_key = format!("{}-{}-{}", alert.title, alert.severity.to_string(),
            alert.transaction.as_ref().map(|tx| &tx.signature).unwrap_or(&alert.id));
        self.cache.insert(cache_key, Instant::now());
    }
}

impl AlertSeverity {
    pub fn to_string(&self) -> String {
        match self {
            AlertSeverity::Low => "Low".to_string(),
            AlertSeverity::Medium => "Medium".to_string(),
            AlertSeverity::High => "High".to_string(),
            AlertSeverity::Critical => "Critical".to_string(),
        }
    }
}

// Helper function to create alert from transaction event
pub fn create_alert_from_transaction(
    tx: &TransactionEvent,
    title: String,
    message: String,
    severity: AlertSeverity,
) -> Alert {
    Alert {
        id: format!("{}-{}", tx.signature, chrono::Utc::now().timestamp()),
        timestamp: chrono::Utc::now().timestamp(),
        severity,
        title,
        message,
        transaction: Some(tx.clone()),
        metadata: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_console_notification() {
        let console = ConsoleChannel::new();
        let alert = Alert {
            id: "test-123".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            severity: AlertSeverity::High,
            title: "Test Alert".to_string(),
            message: "This is a test alert".to_string(),
            transaction: None,
            metadata: HashMap::new(),
        };

        assert!(console.send(&alert).await.is_ok());
    }

    #[test]
    fn test_rate_limiter() {
        let mut limiter = RateLimiter::new();
        
        // Should allow initial requests
        assert!(limiter.check_limit(&AlertSeverity::High));
        
        // Should respect limits
        for _ in 0..19 {
            assert!(limiter.check_limit(&AlertSeverity::High));
        }
        
        // 21st request should be blocked
        assert!(!limiter.check_limit(&AlertSeverity::High));
    }
}