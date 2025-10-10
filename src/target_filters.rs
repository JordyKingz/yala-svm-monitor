use crate::smart_filter::*;
use crate::transaction_extractor::ExtractedTransaction;
use std::env;

/// Create filters specifically for LayerZero and YU Token monitoring
pub fn create_target_address_filters() -> Result<Vec<SmartFilter>, Box<dyn std::error::Error>> {
    let layerzero_address = env::var("LAYERZERO_ADDRESS")?;
    let yu_token_address = env::var("YU_TOKEN_ADDRESS")?;
    
    Ok(vec![
        // LayerZero program interactions
        SmartFilter {
            id: "layerzero_activity".to_string(),
            name: "LayerZero Program Activity".to_string(),
            description: Some("Monitor all LayerZero program interactions".to_string()),
            enabled: true,
            conditions: FilterConditions {
                any_of: Some(vec![
                    FilterCondition::ProgramInvoked {
                        program_id: layerzero_address.clone(),
                    },
                    FilterCondition::AccountInvolved {
                        pubkey: layerzero_address.clone(),
                    },
                ]),
                all_of: None,
                none_of: None,
            },
            actions: vec![
                FilterAction::Alert {
                    severity: AlertLevel::High,
                    channels: vec!["console".to_string(), "database".to_string()],
                },
                FilterAction::Tag {
                    tags: vec!["layerzero".to_string()],
                },
            ],
        },
        
        // YU Token activity
        SmartFilter {
            id: "yu_token_activity".to_string(),
            name: "YU Token Activity".to_string(),
            description: Some("Monitor all YU token transfers and interactions".to_string()),
            enabled: true,
            conditions: FilterConditions {
                any_of: Some(vec![
                    FilterCondition::TokenTransfer {
                        mint: Some(yu_token_address.clone()),
                        operator: ComparisonOperator::GreaterThan,
                        amount: 0.0,
                    },
                    FilterCondition::AccountInvolved {
                        pubkey: yu_token_address.clone(),
                    },
                ]),
                all_of: None,
                none_of: None,
            },
            actions: vec![
                FilterAction::Alert {
                    severity: AlertLevel::High,
                    channels: vec!["console".to_string()],
                },
                FilterAction::Tag {
                    tags: vec!["yu-token".to_string()],
                },
            ],
        },
        
        // Cross-chain message (LayerZero specific)
        SmartFilter {
            id: "layerzero_crosschain".to_string(),
            name: "LayerZero Cross-Chain Messages".to_string(),
            description: Some("Detect cross-chain messaging via LayerZero".to_string()),
            enabled: true,
            conditions: FilterConditions {
                all_of: Some(vec![
                    FilterCondition::ProgramInvoked {
                        program_id: layerzero_address.clone(),
                    },
                    FilterCondition::LogContains {
                        pattern: "cross-chain".to_string(),
                        case_sensitive: false,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                FilterAction::Alert {
                    severity: AlertLevel::Critical,
                    channels: vec!["slack".to_string(), "console".to_string()],
                },
                FilterAction::Store {
                    destination: "crosschain_messages".to_string(),
                },
            ],
        },
        
        // Large YU token transfers
        SmartFilter {
            id: "yu_token_large_transfer".to_string(),
            name: "Large YU Token Transfers".to_string(),
            description: Some("Alert on large YU token movements".to_string()),
            enabled: true,
            conditions: FilterConditions {
                all_of: Some(vec![
                    FilterCondition::TokenTransfer {
                        mint: Some(yu_token_address.clone()),
                        operator: ComparisonOperator::GreaterThan,
                        amount: 1000.0, // Adjust based on token decimals
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                FilterAction::Alert {
                    severity: AlertLevel::Critical,
                    channels: vec!["all".to_string()],
                },
                FilterAction::Tag {
                    tags: vec!["large-transfer".to_string(), "yu-token".to_string()],
                },
            ],
        },
        
        // Failed LayerZero transactions
        SmartFilter {
            id: "layerzero_failed".to_string(),
            name: "Failed LayerZero Transactions".to_string(),
            description: Some("Monitor failed LayerZero transactions for debugging".to_string()),
            enabled: true,
            conditions: FilterConditions {
                all_of: Some(vec![
                    FilterCondition::ProgramInvoked {
                        program_id: layerzero_address.clone(),
                    },
                    FilterCondition::TransactionSuccess { value: false },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                FilterAction::Alert {
                    severity: AlertLevel::Warning,
                    channels: vec!["console".to_string()],
                },
                FilterAction::Store {
                    destination: "failed_layerzero".to_string(),
                },
            ],
        },
        
        // Both addresses involved
        SmartFilter {
            id: "layerzero_yu_interaction".to_string(),
            name: "LayerZero-YU Token Interaction".to_string(),
            description: Some("Transactions involving both LayerZero and YU token".to_string()),
            enabled: true,
            conditions: FilterConditions {
                all_of: Some(vec![
                    FilterCondition::AccountInvolved {
                        pubkey: layerzero_address.clone(),
                    },
                    FilterCondition::AccountInvolved {
                        pubkey: yu_token_address,
                    },
                ]),
                any_of: None,
                none_of: None,
            },
            actions: vec![
                FilterAction::Alert {
                    severity: AlertLevel::Critical,
                    channels: vec!["all".to_string()],
                },
                FilterAction::Tag {
                    tags: vec!["layerzero-yu-interaction".to_string()],
                },
            ],
        },
    ])
}

/// Apply target filters to extracted transactions
pub fn apply_target_filters(
    transactions: &[ExtractedTransaction],
    layerzero_address: &str,
    yu_token_address: &str,
) -> Vec<ExtractedTransaction> {
    transactions.iter()
        .filter(|tx| {
            // Check if transaction involves either target address
            let involves_layerzero = tx.accounts.iter()
                .any(|acc| acc.pubkey == layerzero_address) ||
                tx.instructions.iter()
                .any(|inst| inst.program_id == layerzero_address);
            
            let involves_yu_token = tx.accounts.iter()
                .any(|acc| acc.pubkey == yu_token_address) ||
                tx.token_balance_changes.iter()
                .any(|change| change.mint == yu_token_address);
            
            involves_layerzero || involves_yu_token
        })
        .cloned()
        .collect()
}

/// Create a JSON filter configuration file for target addresses
pub fn export_target_filters_json(output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let filters = create_target_address_filters()?;
    let json = serde_json::to_string_pretty(&filters)?;
    std::fs::write(output_path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_filter_creation() {
        // Set test env vars
        std::env::set_var("LAYERZERO_ADDRESS", "LayerZeroTestAddress123");
        std::env::set_var("YU_TOKEN_ADDRESS", "YUTokenTestAddress456");
        
        let filters = create_target_address_filters().unwrap();
        assert_eq!(filters.len(), 6);
        
        // Clean up
        std::env::remove_var("LAYERZERO_ADDRESS");
        std::env::remove_var("YU_TOKEN_ADDRESS");
    }
}