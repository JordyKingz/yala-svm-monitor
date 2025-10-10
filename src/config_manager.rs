use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn, error};
use crate::filter_engine::{FilterConfig, Action, AlertSeverity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    #[serde(flatten)]
    pub filter: FilterConfig,
    #[serde(default)]
    pub alerts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    pub name: String,
    pub trigger_type: AlertType,
    pub config: AlertConfigDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    Discord,
    Telegram,
    Webhook,
    Email,
    Slack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfigDetails {
    #[serde(flatten)]
    pub connection: HashMap<String, ConfigValue>,
    pub message: MessageTemplate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValue {
    #[serde(rename = "type")]
    pub value_type: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageTemplate {
    pub title: String,
    pub body: String,
}

pub struct ConfigManager {
    monitors_dir: PathBuf,
    alerts_dir: PathBuf,
    pub loaded_monitors: HashMap<String, MonitorConfig>,
    loaded_alerts: HashMap<String, AlertConfig>,
}

impl ConfigManager {
    pub fn new(config_dir: impl AsRef<Path>) -> Self {
        let config_path = config_dir.as_ref();
        Self {
            monitors_dir: config_path.join("monitors"),
            alerts_dir: config_path.join("alerts"),
            loaded_monitors: HashMap::new(),
            loaded_alerts: HashMap::new(),
        }
    }
    
    /// Load all configurations from the config directories
    pub fn load_all(&mut self) -> Result<()> {
        self.load_alerts()?;
        self.load_monitors()?;
        Ok(())
    }
    
    /// Load all alert configurations from config/alerts/
    fn load_alerts(&mut self) -> Result<()> {
        info!("Loading alert configurations from {:?}", self.alerts_dir);
        
        // Create alerts directory if it doesn't exist
        if !self.alerts_dir.exists() {
            std::fs::create_dir_all(&self.alerts_dir)
                .context("Failed to create alerts directory")?;
        }
        
        let entries = std::fs::read_dir(&self.alerts_dir)
            .context("Failed to read alerts directory")?;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match self.load_alert_file(&path) {
                    Ok(count) => info!("Loaded {} alerts from {:?}", count, path.file_name().unwrap()),
                    Err(e) => error!("Failed to load alerts from {:?}: {}", path, e),
                }
            }
        }
        
        info!("Loaded {} total alert configurations", self.loaded_alerts.len());
        Ok(())
    }
    
    /// Load alerts from a single JSON file
    fn load_alert_file(&mut self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read alert file")?;
        
        let alerts: HashMap<String, AlertConfig> = serde_json::from_str(&content)
            .context("Failed to parse alert JSON")?;
        
        let count = alerts.len();
        self.loaded_alerts.extend(alerts);
        Ok(count)
    }
    
    /// Load all monitor configurations from config/monitors/
    fn load_monitors(&mut self) -> Result<()> {
        info!("Loading monitor configurations from {:?}", self.monitors_dir);
        
        if !self.monitors_dir.exists() {
            return Err(anyhow::anyhow!("Monitors directory does not exist: {:?}", self.monitors_dir));
        }
        
        let entries = std::fs::read_dir(&self.monitors_dir)
            .context("Failed to read monitors directory")?;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match self.load_monitor_file(&path) {
                    Ok(count) => info!("Loaded {} monitors from {:?}", count, path.file_name().unwrap()),
                    Err(e) => error!("Failed to load monitors from {:?}: {}", path, e),
                }
            }
        }
        
        info!("Loaded {} total monitor configurations", self.loaded_monitors.len());
        Ok(())
    }
    
    /// Load monitors from a single JSON file
    fn load_monitor_file(&mut self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read monitor file")?;
        
        let monitors: Vec<MonitorConfig> = serde_json::from_str(&content)
            .context("Failed to parse monitor JSON")?;
        
        let count = monitors.len();
        
        for monitor in monitors {
            self.loaded_monitors.insert(monitor.filter.id.clone(), monitor);
        }
        
        Ok(count)
    }
    
    /// Get all filter configurations with resolved alert actions
    pub fn get_filters_with_alerts(&self) -> Result<Vec<FilterConfig>> {
        let mut filters = Vec::new();
        
        for (id, monitor) in &self.loaded_monitors {
            let mut filter = monitor.filter.clone();
            
            // Add alert actions based on configured alerts
            for alert_id in &monitor.alerts {
                if let Some(alert_config) = self.loaded_alerts.get(alert_id) {
                    let action = self.create_action_from_alert(alert_config)?;
                    filter.actions.push(action);
                } else {
                    warn!("Alert '{}' referenced in monitor '{}' not found", alert_id, id);
                }
            }
            
            filters.push(filter);
        }
        
        Ok(filters)
    }
    
    /// Create an Action from an AlertConfig
    fn create_action_from_alert(&self, alert: &AlertConfig) -> Result<Action> {
        match alert.trigger_type {
            AlertType::Discord => Ok(Action::Webhook {
                url: alert.config.connection.get("discord_url")
                    .and_then(|v| Some(v.value.clone()))
                    .ok_or_else(|| anyhow::anyhow!("Discord alert missing discord_url"))?,
                method: "POST".to_string(),
            }),
            AlertType::Telegram => Ok(Action::Alert {
                severity: AlertSeverity::High,
                channels: vec!["telegram".to_string()],
            }),
            AlertType::Webhook => Ok(Action::Webhook {
                url: alert.config.connection.get("webhook_url")
                    .and_then(|v| Some(v.value.clone()))
                    .ok_or_else(|| anyhow::anyhow!("Webhook alert missing webhook_url"))?,
                method: alert.config.connection.get("method")
                    .and_then(|v| Some(v.value.clone()))
                    .unwrap_or_else(|| "POST".to_string()),
            }),
            AlertType::Email => {
                warn!("Email alerts not yet implemented");
                Ok(Action::Log {
                    level: "info".to_string(),
                    message: format!("Email alert: {}", alert.name),
                })
            }
            AlertType::Slack => Ok(Action::Alert {
                severity: AlertSeverity::High,
                channels: vec!["slack".to_string()],
            }),
        }
    }
    
    /// Get alert configuration by ID
    pub fn get_alert(&self, alert_id: &str) -> Option<&AlertConfig> {
        self.loaded_alerts.get(alert_id)
    }
    
    /// Format a message template with transaction data
    pub fn format_message(
        template: &MessageTemplate,
        transaction_data: &serde_json::Value,
    ) -> (String, String) {
        let title = replace_placeholders(&template.title, transaction_data);
        let body = replace_placeholders(&template.body, transaction_data);
        (title, body)
    }
}

/// Format a message template with transaction data (standalone function)
pub fn format_message(
    template: &MessageTemplate,
    transaction_data: &serde_json::Value,
) -> (String, String) {
    let title = replace_placeholders(&template.title, transaction_data);
    let body = replace_placeholders(&template.body, transaction_data);
    (title, body)
}

/// Replace ${...} placeholders in template with actual values
pub fn replace_placeholders(template: &str, data: &serde_json::Value) -> String {
    let mut result = template.to_string();
    
    // Find all placeholders
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    
    for cap in re.captures_iter(template) {
        if let Some(path) = cap.get(1) {
            let path_str = path.as_str();
            if let Some(value) = get_json_value(data, path_str) {
                // Don't format slot numbers or signatures
                let formatted_value = if path_str == "slot" || path_str.contains("signature") {
                    value.to_string().trim_matches('"').to_string()
                } else {
                    value_to_string(&value)
                };
                result = result.replace(&cap[0], &formatted_value);
            }
        }
    }
    
    result
}

/// Get value from JSON using dot notation path
fn get_json_value<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = data;
    
    for part in parts {
        // Handle array index notation like events.0
        if let Ok(index) = part.parse::<usize>() {
            current = current.get(index)?;
        } else {
            current = current.get(part)?;
        }
    }
    
    Some(current)
}

/// Convert JSON value to string for template replacement
fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            // Format large numbers with comma separators
            if let Some(f) = n.as_f64() {
                if f.abs() >= 1000.0 {
                    format_number(f)
                } else {
                    n.to_string()
                }
            } else {
                n.to_string()
            }
        },
        Value::Bool(b) => b.to_string(),
        _ => value.to_string(),
    }
}

/// Format number with comma separators and handle negatives for burns
fn format_number(num: f64) -> String {
    let abs_num = num.abs();
    let formatted = if abs_num >= 1_000_000.0 {
        format!("{:.2}M", abs_num / 1_000_000.0)
    } else if abs_num >= 1_000.0 {
        format!("{:.2}K", abs_num / 1_000.0)
    } else {
        format!("{:.2}", abs_num)
    };
    formatted
}