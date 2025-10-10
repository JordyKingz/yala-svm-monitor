use anyhow::{Result, Context};
use reqwest;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};
use crate::transaction_extractor::ExtractedTransaction;
use crate::filter_engine::{AlertSeverity, MatchedFilter};

#[derive(Debug, Clone)]
pub struct TelegramNotifier {
    bot_token: String,
    chat_id: String,
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest {
    chat_id: String,
    text: String,
    parse_mode: String,
    disable_web_page_preview: bool,
}

#[derive(Debug, Deserialize)]
struct TelegramResponse {
    ok: bool,
    description: Option<String>,
}

impl TelegramNotifier {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        let client = reqwest::Client::new();
        let base_url = format!("https://api.telegram.org/bot{}", bot_token);
        
        Self {
            bot_token,
            chat_id,
            client,
            base_url,
        }
    }
    
    pub async fn send_alert(
        &self,
        transaction: &ExtractedTransaction,
        matched_filter: &MatchedFilter,
        severity: &AlertSeverity,
    ) -> Result<()> {
        let message = self.format_alert_message(transaction, matched_filter, severity);
        self.send_message(&message).await
    }
    
    pub async fn send_custom_message(&self, title: &str, body: &str) -> Result<()> {
        let full_message = format!("<b>{}</b>\n\n{}", title, body);
        self.send_message(&full_message).await
    }
    
    pub async fn send_message(&self, text: &str) -> Result<()> {
        let url = format!("{}/sendMessage", self.base_url);
        
        let request = SendMessageRequest {
            chat_id: self.chat_id.clone(),
            text: text.to_string(),
            parse_mode: "HTML".to_string(),
            disable_web_page_preview: true,
        };
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send Telegram message")?;
        
        let telegram_response: TelegramResponse = response
            .json()
            .await
            .context("Failed to parse Telegram response")?;
        
        if !telegram_response.ok {
            let error_msg = telegram_response.description
                .unwrap_or_else(|| "Unknown error".to_string());
            return Err(anyhow::anyhow!("Telegram API error: {}", error_msg));
        }
        
        info!("Successfully sent Telegram notification");
        Ok(())
    }
    
    fn format_alert_message(
        &self,
        transaction: &ExtractedTransaction,
        matched_filter: &MatchedFilter,
        severity: &AlertSeverity,
    ) -> String {
        let severity_emoji = match severity {
            AlertSeverity::Low => "ℹ️",
            AlertSeverity::Medium => "⚠️",
            AlertSeverity::High => "🚨",
            AlertSeverity::Critical => "🔴",
        };
        
        let mut message = format!(
            "{} <b>{}</b>\n\n",
            severity_emoji,
            html_escape(&matched_filter.filter_name)
        );
        
        // Transaction details
        message.push_str(&format!(
            "📝 <b>Transaction Details</b>\n",
        ));
        message.push_str(&format!(
            "• Signature: <code>{}</code>\n",
            &transaction.signature[..20]
        ));
        message.push_str(&format!(
            "• Slot: {}\n",
            transaction.slot
        ));
        message.push_str(&format!(
            "• Status: {}\n",
            if transaction.success { "✅ Success" } else { "❌ Failed" }
        ));
        message.push_str(&format!(
            "• Fee: {} SOL\n\n",
            transaction.fee as f64 / 1_000_000_000.0
        ));
        
        // Token balance changes
        if !transaction.token_balance_changes.is_empty() {
            message.push_str("💰 <b>Token Balance Changes</b>\n");
            for change in &transaction.token_balance_changes {
                if change.change.abs() > 0.0 {
                    let direction = if change.change > 0.0 { "+" } else { "" };
                    message.push_str(&format!(
                        "• {}{:.2} tokens\n",
                        direction, change.change
                    ));
                    
                    // Add mint address for context
                    if let Some(yuya_address) = std::env::var("YU_TOKEN_ADDRESS").ok() {
                        if change.mint == yuya_address {
                            message.push_str("  🪙 <i>YU Token</i>\n");
                        }
                    }
                }
            }
            message.push_str("\n");
        }
        
        // Programs involved
        if !transaction.instructions.is_empty() {
            message.push_str("🔧 <b>Programs</b>\n");
            let unique_programs: std::collections::HashSet<_> = transaction.instructions
                .iter()
                .map(|inst| &inst.program_id)
                .collect();
            
            for (i, program) in unique_programs.iter().take(3).enumerate() {
                message.push_str(&format!(
                    "• <code>{}</code>\n",
                    &program[..8]
                ));
            }
            if unique_programs.len() > 3 {
                message.push_str(&format!("• <i>...and {} more</i>\n", unique_programs.len() - 3));
            }
            message.push_str("\n");
        }
        
        // Explorer link
        message.push_str(&format!(
            "🔍 <a href=\"https://solscan.io/tx/{}\">View on Solscan</a>",
            transaction.signature
        ));
        
        message
    }
}

// HTML escape function to prevent injection
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// Telegram bot setup instructions
pub fn print_telegram_setup_instructions() {
    println!(r#"
📱 Telegram Bot Setup Instructions:

1. Create a new bot:
   - Message @BotFather on Telegram
   - Send /newbot
   - Choose a name and username for your bot
   - Copy the bot token

2. Get your chat ID:
   - Start a chat with your bot
   - Send any message
   - Visit: https://api.telegram.org/bot<YOUR_BOT_TOKEN>/getUpdates
   - Find your chat ID in the response

3. Set environment variables:
   TELEGRAM_BOT_TOKEN=your_bot_token_here
   TELEGRAM_CHAT_ID=your_chat_id_here

4. Test the connection:
   The bot will send a test message on startup
"#);
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("Test <script>"), "Test &lt;script&gt;");
        assert_eq!(html_escape("Test & Co"), "Test &amp; Co");
    }
}