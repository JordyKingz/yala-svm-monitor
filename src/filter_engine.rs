use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn, debug};
use crate::transaction_extractor::ExtractedTransaction;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub conditions: ConditionSet,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionSet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<Condition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<Condition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub none_of: Option<Vec<Condition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Condition {
    ProgramInvoked {
        program_id: String,
    },
    TokenTransfer {
        mint: Option<String>,
        operator: ComparisonOperator,
        amount: f64,
    },
    TokenMint {
        mint: String,
        operator: ComparisonOperator,
        amount: f64,
    },
    TokenBurn {
        mint: String,
        operator: ComparisonOperator,
        amount: f64,
    },
    BalanceChange {
        account: Option<String>,
        operator: ComparisonOperator,
        amount: f64,
    },
    TransactionStatus {
        success: bool,
    },
    FeeAmount {
        operator: ComparisonOperator,
        amount: u64,
    },
    InstructionCount {
        operator: ComparisonOperator,
        count: usize,
    },
    AccountInvolved {
        account: String,
    },
    LogContains {
        pattern: String,
        case_sensitive: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComparisonOperator {
    GreaterThan,
    LessThan,
    Equal,
    GreaterThanOrEqual,
    LessThanOrEqual,
    NotEqual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    Alert {
        severity: AlertSeverity,
        channels: Vec<String>,
    },
    Store {
        collection: String,
    },
    Webhook {
        url: String,
        method: String,
    },
    Log {
        level: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}

pub struct FilterEngine {
    filters: Vec<FilterConfig>,
}

impl FilterEngine {
    pub fn new(filters: Vec<FilterConfig>) -> Self {
        let enabled_filters: Vec<FilterConfig> = filters
            .into_iter()
            .filter(|f| f.enabled)
            .collect();
        
        info!("Initialized filter engine with {} active filters", enabled_filters.len());
        Self { filters: enabled_filters }
    }
    
    pub fn from_json_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read filter configuration file")?;
        let filters: Vec<FilterConfig> = serde_json::from_str(&content)
            .context("Failed to parse filter configuration")?;
        Ok(Self::new(filters))
    }
    
    pub fn evaluate_transaction(&self, transaction: &ExtractedTransaction) -> Vec<MatchedFilter> {
        let mut matched_filters = Vec::new();
        
        for filter in &self.filters {
            if self.evaluate_condition_set(&filter.conditions, transaction) {
                debug!("Transaction {} matched filter: {}", transaction.signature, filter.name);
                matched_filters.push(MatchedFilter {
                    filter_id: filter.id.clone(),
                    filter_name: filter.name.clone(),
                    actions: filter.actions.clone(),
                });
            }
        }
        
        matched_filters
    }
    
    fn evaluate_condition_set(&self, conditions: &ConditionSet, transaction: &ExtractedTransaction) -> bool {
        let mut result = true;
        
        // Check all_of conditions (AND logic)
        if let Some(all_conditions) = &conditions.all_of {
            result = all_conditions.iter()
                .all(|cond| self.evaluate_condition(cond, transaction));
        }
        
        // Check any_of conditions (OR logic)
        if let Some(any_conditions) = &conditions.any_of {
            let any_match = any_conditions.iter()
                .any(|cond| self.evaluate_condition(cond, transaction));
            result = result && any_match;
        }
        
        // Check none_of conditions (NOT logic)
        if let Some(none_conditions) = &conditions.none_of {
            let none_match = !none_conditions.iter()
                .any(|cond| self.evaluate_condition(cond, transaction));
            result = result && none_match;
        }
        
        result
    }
    
    fn evaluate_condition(&self, condition: &Condition, transaction: &ExtractedTransaction) -> bool {
        match condition {
            Condition::ProgramInvoked { program_id } => {
                // Check both top-level instructions and inner instructions
                let in_main_instructions = transaction.instructions.iter()
                    .any(|inst| inst.program_id == *program_id);
                    
                let in_inner_instructions = transaction.inner_instructions.iter()
                    .any(|inner_set| {
                        inner_set.instructions.iter()
                            .any(|inst| inst.program_id == *program_id)
                    });
                    
                in_main_instructions || in_inner_instructions
            },
            
            Condition::TokenTransfer { mint, operator, amount } => {
                transaction.token_balance_changes.iter()
                    .any(|change| {
                        let mint_match = mint.as_ref().map_or(true, |m| change.mint == *m);
                        let amount_match = self.compare_f64(change.change.abs(), *amount, operator);
                        mint_match && amount_match && change.change != 0.0
                    })
            },
            
            Condition::TokenMint { mint, operator, amount } => {
                // Check for mint operations (tokens created from nothing)
                let result = transaction.token_balance_changes.iter()
                    .any(|change| {
                        if change.mint != *mint {
                            return false;
                        }
                        
                        debug!("Checking TokenMint condition for mint {} with change {}", mint, change.change);
                        
                        // Mint operation: positive change and either:
                        // 1. instruction type contains "mint", OR
                        // 2. before amount was 0 (new token account), OR  
                        // 3. logs contain "MintTo" or "mint"
                        let has_mint_instruction = transaction.instructions.iter().any(|inst| {
                            inst.instruction_type.as_ref()
                                .map_or(false, |t| t.contains("mint"))
                        });
                        
                        let is_new_account = change.before.ui_amount.unwrap_or(0.0) == 0.0;
                        
                        let has_mint_log = transaction.log_messages.iter()
                            .any(|log| log.contains("MintTo") || log.contains("mint"));
                        
                        let is_mint = change.change > 0.0 && (has_mint_instruction || is_new_account || has_mint_log);
                        
                        if is_mint {
                            debug!("Found mint operation with amount {}, comparing with {}", change.change, amount);
                            debug!("  has_mint_instruction: {}, is_new_account: {}, has_mint_log: {}", 
                                has_mint_instruction, is_new_account, has_mint_log);
                        }
                        
                        is_mint && self.compare_f64(change.change, *amount, operator)
                    });
                    
                if !result && transaction.token_balance_changes.iter().any(|c| c.mint == *mint) {
                    debug!("TokenMint condition failed for mint {} despite having balance changes", mint);
                }
                
                result
            },
            
            Condition::TokenBurn { mint, operator, amount } => {
                // Check for burn operations (tokens destroyed)
                let result = transaction.token_balance_changes.iter()
                    .any(|change| {
                        if change.mint != *mint {
                            return false;
                        }
                        
                        debug!("Checking TokenBurn condition for mint {} with change {}", mint, change.change);
                        
                        // Burn operation: negative change and either:
                        // 1. instruction type contains "burn", OR
                        // 2. tokens are sent to a known burn address, OR  
                        // 3. logs contain "Burn" or "burn"
                        let has_burn_instruction = transaction.instructions.iter().any(|inst| {
                            inst.instruction_type.as_ref()
                                .map_or(false, |t| t.contains("burn"))
                        });
                        
                        let has_burn_log = transaction.log_messages.iter()
                            .any(|log| log.contains("Burn") || log.contains("burn"));
                        
                        let is_burn = change.change < 0.0 && (has_burn_instruction || has_burn_log);
                        
                        if is_burn {
                            debug!("Found burn operation with amount {}, comparing with {}", change.change.abs(), amount);
                            debug!("  has_burn_instruction: {}, has_burn_log: {}", 
                                has_burn_instruction, has_burn_log);
                        }
                        
                        is_burn && self.compare_f64(change.change.abs(), *amount, operator)
                    });
                    
                if !result && transaction.token_balance_changes.iter().any(|c| c.mint == *mint) {
                    debug!("TokenBurn condition failed for mint {} despite having balance changes", mint);
                }
                
                result
            },
            
            Condition::BalanceChange { account, operator, amount } => {
                let changes_to_check: Vec<&crate::transaction_extractor::BalanceChange> = if let Some(acc) = account {
                    transaction.balance_changes.values()
                        .filter(|change| change.account == *acc)
                        .collect()
                } else {
                    transaction.balance_changes.values().collect()
                };
                
                changes_to_check.iter()
                    .any(|change| {
                        let amount_in_sol = (*amount * 1_000_000_000.0) as i64;
                        self.compare_i64(change.change.abs(), amount_in_sol.abs(), operator)
                    })
            },
            
            Condition::TransactionStatus { success } => {
                transaction.success == *success
            },
            
            Condition::FeeAmount { operator, amount } => {
                self.compare_u64(transaction.fee, *amount, operator)
            },
            
            Condition::InstructionCount { operator, count } => {
                self.compare_usize(transaction.instructions.len(), *count, operator)
            },
            
            Condition::AccountInvolved { account } => {
                transaction.accounts.iter()
                    .any(|acc| acc.pubkey == *account)
            },
            
            Condition::LogContains { pattern, case_sensitive } => {
                if *case_sensitive {
                    transaction.log_messages.iter()
                        .any(|log| log.contains(pattern))
                } else {
                    let pattern_lower = pattern.to_lowercase();
                    transaction.log_messages.iter()
                        .any(|log| log.to_lowercase().contains(&pattern_lower))
                }
            },
        }
    }
    
    fn compare_f64(&self, value: f64, target: f64, operator: &ComparisonOperator) -> bool {
        match operator {
            ComparisonOperator::GreaterThan => value > target,
            ComparisonOperator::LessThan => value < target,
            ComparisonOperator::Equal => (value - target).abs() < f64::EPSILON,
            ComparisonOperator::GreaterThanOrEqual => value >= target,
            ComparisonOperator::LessThanOrEqual => value <= target,
            ComparisonOperator::NotEqual => (value - target).abs() >= f64::EPSILON,
        }
    }
    
    fn compare_i64(&self, value: i64, target: i64, operator: &ComparisonOperator) -> bool {
        match operator {
            ComparisonOperator::GreaterThan => value > target,
            ComparisonOperator::LessThan => value < target,
            ComparisonOperator::Equal => value == target,
            ComparisonOperator::GreaterThanOrEqual => value >= target,
            ComparisonOperator::LessThanOrEqual => value <= target,
            ComparisonOperator::NotEqual => value != target,
        }
    }
    
    fn compare_u64(&self, value: u64, target: u64, operator: &ComparisonOperator) -> bool {
        match operator {
            ComparisonOperator::GreaterThan => value > target,
            ComparisonOperator::LessThan => value < target,
            ComparisonOperator::Equal => value == target,
            ComparisonOperator::GreaterThanOrEqual => value >= target,
            ComparisonOperator::LessThanOrEqual => value <= target,
            ComparisonOperator::NotEqual => value != target,
        }
    }
    
    fn compare_usize(&self, value: usize, target: usize, operator: &ComparisonOperator) -> bool {
        match operator {
            ComparisonOperator::GreaterThan => value > target,
            ComparisonOperator::LessThan => value < target,
            ComparisonOperator::Equal => value == target,
            ComparisonOperator::GreaterThanOrEqual => value >= target,
            ComparisonOperator::LessThanOrEqual => value <= target,
            ComparisonOperator::NotEqual => value != target,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchedFilter {
    pub filter_id: String,
    pub filter_name: String,
    pub actions: Vec<Action>,
}

// Helper function to create default YUYA mint filters
pub fn create_yuya_mint_filters(yuya_mint_address: &str) -> Vec<FilterConfig> {
    vec![
        // Mint filters
        FilterConfig {
            id: "yuya_mint_30m".to_string(),
            name: "YUYA Token Mint >= 30M".to_string(),
            enabled: true,
            conditions: ConditionSet {
                all_of: Some(vec![
                    Condition::TokenMint {
                        mint: yuya_mint_address.to_string(),
                        operator: ComparisonOperator::GreaterThanOrEqual,
                        amount: 30_000_000.0,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                Action::Alert {
                    severity: AlertSeverity::Critical,
                    channels: vec!["telegram".to_string(), "database".to_string()],
                },
                Action::Store {
                    collection: "critical_mints".to_string(),
                },
            ],
        },
        FilterConfig {
            id: "yuya_mint_10m".to_string(),
            name: "YUYA Token Mint >= 10M".to_string(),
            enabled: true,
            conditions: ConditionSet {
                all_of: Some(vec![
                    Condition::TokenMint {
                        mint: yuya_mint_address.to_string(),
                        operator: ComparisonOperator::GreaterThanOrEqual,
                        amount: 10_000_000.0,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                Action::Alert {
                    severity: AlertSeverity::High,
                    channels: vec!["telegram".to_string(), "database".to_string()],
                },
                Action::Store {
                    collection: "large_mints".to_string(),
                },
            ],
        },
        FilterConfig {
            id: "yuya_mint_1m".to_string(),
            name: "YUYA Token Mint >= 1M".to_string(),
            enabled: true,
            conditions: ConditionSet {
                all_of: Some(vec![
                    Condition::TokenMint {
                        mint: yuya_mint_address.to_string(),
                        operator: ComparisonOperator::GreaterThanOrEqual,
                        amount: 1_000_000.0,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                Action::Alert {
                    severity: AlertSeverity::Medium,
                    channels: vec!["database".to_string()],
                },
                Action::Store {
                    collection: "medium_mints".to_string(),
                },
            ],
        },
        // Burn filters
        FilterConfig {
            id: "yuya_burn_10m".to_string(),
            name: "YUYA Token Burn >= 10M".to_string(),
            enabled: true,
            conditions: ConditionSet {
                all_of: Some(vec![
                    Condition::TokenBurn {
                        mint: yuya_mint_address.to_string(),
                        operator: ComparisonOperator::GreaterThanOrEqual,
                        amount: 10_000_000.0,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                Action::Alert {
                    severity: AlertSeverity::Critical,
                    channels: vec!["telegram".to_string(), "database".to_string()],
                },
                Action::Store {
                    collection: "large_burns".to_string(),
                },
            ],
        },
        FilterConfig {
            id: "yuya_burn_1m".to_string(),
            name: "YUYA Token Burn >= 1M".to_string(),
            enabled: true,
            conditions: ConditionSet {
                all_of: Some(vec![
                    Condition::TokenBurn {
                        mint: yuya_mint_address.to_string(),
                        operator: ComparisonOperator::GreaterThanOrEqual,
                        amount: 1_000_000.0,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                Action::Alert {
                    severity: AlertSeverity::High,
                    channels: vec!["telegram".to_string(), "database".to_string()],
                },
                Action::Store {
                    collection: "medium_burns".to_string(),
                },
            ],
        },
    ]
}