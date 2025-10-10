use anyhow::Result;
use reqwest;
use serde::{Deserialize, Serialize};
use std::env;
use tracing::{info, error};

#[derive(Debug, Clone)]
pub struct SlackNotifier {
    webhook_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackMessage {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<SlackAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<SlackBlock>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackAttachment {
    pub color: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<SlackField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackField {
    pub title: String,
    pub value: String,
    pub short: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SlackBlock {
    #[serde(rename = "section")]
    Section {
        text: SlackText,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<SlackText>>,
    },
    #[serde(rename = "actions")]
    Actions {
        elements: Vec<SlackElement>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackText {
    #[serde(rename = "type")]
    pub text_type: String,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SlackElement {
    #[serde(rename = "button")]
    Button {
        text: SlackButtonText,
        url: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SlackButtonText {
    #[serde(rename = "type")]
    pub text_type: String,
    pub text: String,
}

impl SlackNotifier {
    pub fn new() -> Result<Self> {
        let webhook_url = env::var("SLACK_WEBHOOK_URL")
            .map_err(|_| anyhow::anyhow!("SLACK_WEBHOOK_URL environment variable not set"))?;
        
        let client = reqwest::Client::new();
        
        Ok(Self {
            webhook_url,
            client,
        })
    }
    
    pub fn from_url(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: reqwest::Client::new(),
        }
    }
    
    pub async fn send_message(&self, message: SlackMessage) -> Result<()> {
        let response = self.client
            .post(&self.webhook_url)
            .json(&message)
            .send()
            .await?;
        
        if response.status().is_success() {
            info!("Slack notification sent successfully");
            Ok(())
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            error!("Failed to send Slack notification: {} - {}", status, error_text);
            Err(anyhow::anyhow!("Slack notification failed: {} - {}", status, error_text))
        }
    }
    
    pub async fn send_simple_message(&self, text: &str) -> Result<()> {
        let message = SlackMessage {
            text: text.to_string(),
            attachments: None,
            blocks: None,
        };
        self.send_message(message).await
    }
    
    pub async fn send_transaction_alert(
        &self,
        title: &str,
        signature: &str,
        slot: u64,
        amount: Option<f64>,
        token: Option<&str>,
        extra_fields: Vec<(String, String)>,
    ) -> Result<()> {
        let mut fields = vec![];
        
        // Add amount field if provided
        if let Some(amount) = amount {
            let token_str = token.unwrap_or("tokens");
            fields.push(SlackField {
                title: "Amount".to_string(),
                value: format!("{} {}", amount, token_str),
                short: true,
            });
        }
        
        // Add slot field
        fields.push(SlackField {
            title: "Slot".to_string(),
            value: slot.to_string(),
            short: true,
        });
        
        // Add extra fields
        for (title, value) in extra_fields {
            fields.push(SlackField {
                title,
                value,
                short: true,
            });
        }
        
        // Add transaction field
        fields.push(SlackField {
            title: "Transaction".to_string(),
            value: format!("<https://solscan.io/tx/{}|{}>", signature, &signature[..20]),
            short: false,
        });
        
        let attachment = SlackAttachment {
            color: self.get_color_for_title(title),
            title: title.to_string(),
            fields: Some(fields),
            text: None,
        };
        
        let message = SlackMessage {
            text: title.to_string(),
            attachments: Some(vec![attachment]),
            blocks: None,
        };
        
        self.send_message(message).await
    }
    
    fn get_color_for_title(&self, title: &str) -> String {
        if title.contains("Mint") {
            "danger".to_string()  // Red
        } else if title.contains("Burn") {
            "#ff9500".to_string() // Orange
        } else if title.contains("Bridge") {
            "#1890ff".to_string() // Blue
        } else if title.contains("Swap") {
            "good".to_string()    // Green
        } else {
            "warning".to_string() // Yellow
        }
    }
}

// Convenience function for sending quick alerts
pub async fn send_slack_alert(title: &str, body: &str) -> Result<()> {
    let notifier = SlackNotifier::new()?;
    
    let attachment = SlackAttachment {
        color: "warning".to_string(),
        title: title.to_string(),
        text: Some(body.to_string()),
        fields: None,
    };
    
    let message = SlackMessage {
        text: title.to_string(),
        attachments: Some(vec![attachment]),
        blocks: None,
    };
    
    notifier.send_message(message).await
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_slack_message_serialization() {
        let message = SlackMessage {
            text: "Test message".to_string(),
            attachments: Some(vec![
                SlackAttachment {
                    color: "good".to_string(),
                    title: "Test Title".to_string(),
                    fields: Some(vec![
                        SlackField {
                            title: "Field 1".to_string(),
                            value: "Value 1".to_string(),
                            short: true,
                        }
                    ]),
                    text: None,
                }
            ]),
            blocks: None,
        };
        
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("Test message"));
        assert!(json.contains("Test Title"));
    }
}