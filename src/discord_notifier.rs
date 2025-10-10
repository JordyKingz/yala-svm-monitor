use anyhow::{Result, Context};
use reqwest;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};
use crate::transaction_extractor::ExtractedTransaction;
use crate::config_manager::MessageTemplate;

#[derive(Debug, Clone)]
pub struct DiscordNotifier {
    webhook_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct DiscordWebhookPayload {
    content: Option<String>,
    embeds: Vec<DiscordEmbed>,
}

#[derive(Debug, Serialize)]
struct DiscordEmbed {
    title: Option<String>,
    description: Option<String>,
    color: Option<i32>,
    fields: Vec<DiscordEmbedField>,
    footer: Option<DiscordEmbedFooter>,
    timestamp: Option<String>,
}

#[derive(Debug, Serialize)]
struct DiscordEmbedField {
    name: String,
    value: String,
    inline: bool,
}

#[derive(Debug, Serialize)]
struct DiscordEmbedFooter {
    text: String,
}

impl DiscordNotifier {
    pub fn new(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: reqwest::Client::new(),
        }
    }
    
    pub async fn send_transaction_alert(
        &self,
        transaction: &ExtractedTransaction,
        filter_name: &str,
        template: Option<&MessageTemplate>,
    ) -> Result<()> {
        let payload = if let Some(tmpl) = template {
            self.create_payload_from_template(transaction, tmpl)
        } else {
            self.create_default_payload(transaction, filter_name)
        };
        
        self.send_webhook(payload).await
    }
    
    async fn send_webhook(&self, payload: DiscordWebhookPayload) -> Result<()> {
        let response = self.client
            .post(&self.webhook_url)
            .json(&payload)
            .send()
            .await
            .context("Failed to send Discord webhook")?;
        
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("Discord webhook failed with status {}: {}", status, error_text));
        }
        
        info!("Successfully sent Discord notification");
        Ok(())
    }
    
    fn create_payload_from_template(
        &self,
        transaction: &ExtractedTransaction,
        template: &MessageTemplate,
    ) -> DiscordWebhookPayload {
        // Convert transaction to JSON for template processing
        let transaction_json = serde_json::to_value(transaction).unwrap_or_default();
        let (title, body) = crate::config_manager::format_message(template, &transaction_json);
        
        // Determine color based on transaction type
        let color = if title.contains("Mint") {
            0x00FF00 // Green for mints
        } else if title.contains("Burn") {
            0xFF4500 // Orange-red for burns
        } else if title.contains("Swap") {
            0x1E90FF // Blue for swaps
        } else {
            0xFFD700 // Gold for others
        };
        
        DiscordWebhookPayload {
            content: None,
            embeds: vec![DiscordEmbed {
                title: Some(title),
                description: Some(body),
                color: Some(color),
                fields: vec![],
                footer: Some(DiscordEmbedFooter {
                    text: "Solana Transaction Monitor".to_string(),
                }),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            }],
        }
    }
    
    fn create_default_payload(
        &self,
        transaction: &ExtractedTransaction,
        filter_name: &str,
    ) -> DiscordWebhookPayload {
        let mut fields = vec![
            DiscordEmbedField {
                name: "Filter".to_string(),
                value: filter_name.to_string(),
                inline: true,
            },
            DiscordEmbedField {
                name: "Slot".to_string(),
                value: transaction.slot.to_string(),
                inline: true,
            },
            DiscordEmbedField {
                name: "Status".to_string(),
                value: if transaction.success { "✅ Success" } else { "❌ Failed" }.to_string(),
                inline: true,
            },
            DiscordEmbedField {
                name: "Fee".to_string(),
                value: format!("{:.6} SOL", transaction.fee as f64 / 1_000_000_000.0),
                inline: true,
            },
        ];
        
        // Add token balance changes
        for (i, change) in transaction.token_balance_changes.iter().take(3).enumerate() {
            if change.change.abs() > 0.0 {
                let direction = if change.change > 0.0 { "+" } else { "" };
                fields.push(DiscordEmbedField {
                    name: format!("Token Change #{}", i + 1),
                    value: format!("{}{:.2} tokens", direction, change.change),
                    inline: true,
                });
            }
        }
        
        // Add signature
        fields.push(DiscordEmbedField {
            name: "Signature".to_string(),
            value: format!("```{}```", &transaction.signature[..20]),
            inline: false,
        });
        
        // Add explorer link
        fields.push(DiscordEmbedField {
            name: "Explorer".to_string(),
            value: format!("[View on Solscan](https://solscan.io/tx/{})", transaction.signature),
            inline: false,
        });
        
        DiscordWebhookPayload {
            content: None,
            embeds: vec![DiscordEmbed {
                title: Some(format!("🚨 {} Alert", filter_name)),
                description: Some("A transaction matched your monitoring criteria.".to_string()),
                color: Some(0xFF0000), // Red
                fields,
                footer: Some(DiscordEmbedFooter {
                    text: "Solana Monitor".to_string(),
                }),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            }],
        }
    }
}