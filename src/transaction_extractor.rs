use anyhow::{Result, Context};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::Signature,
};
use solana_transaction_status::{
    UiTransactionEncoding,
    EncodedTransactionWithStatusMeta,
    EncodedTransaction,
    UiMessage,
    UiParsedMessage,
    UiCompiledInstruction,
    UiInstruction,
    UiParsedInstruction,
    UiPartiallyDecodedInstruction,
    UiAccountsList,
    UiTransactionTokenBalance,
    option_serializer::OptionSerializer,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn, error, debug};
use crate::rpc_client_with_failover::RpcClientWithFailover;
use std::sync::Arc;

/// Comprehensive transaction data structure capturing all available information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedTransaction {
    // Basic Information
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub block_height: Option<u64>,
    
    // Transaction Status
    pub success: bool,
    pub fee: u64,
    pub error: Option<String>,
    pub compute_units_consumed: Option<u64>,
    
    // Accounts Information
    pub accounts: Vec<AccountInfo>,
    pub account_keys: Vec<String>,
    pub static_account_keys: Vec<String>,
    pub writable_account_indices: Vec<u8>,
    pub readonly_account_indices: Vec<u8>,
    
    // Balance Changes
    pub pre_balances: Vec<u64>,
    pub post_balances: Vec<u64>,
    pub balance_changes: HashMap<String, BalanceChange>,
    
    // Token Balances
    pub pre_token_balances: Vec<TokenBalance>,
    pub post_token_balances: Vec<TokenBalance>,
    pub token_balance_changes: Vec<TokenBalanceChange>,
    
    // Instructions
    pub instructions: Vec<ExtractedInstruction>,
    pub inner_instructions: Vec<InnerInstructionSet>,
    
    // Logs and Messages
    pub log_messages: Vec<String>,
    pub return_data: Option<ReturnData>,
    
    // Address Lookup Tables
    pub address_table_lookups: Vec<AddressTableLookup>,
    
    // Version and other metadata
    pub version: String,
    pub recent_blockhash: String,
    pub loaded_addresses: LoadedAddresses,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub pubkey: String,
    pub is_signer: bool,
    pub is_writable: bool,
    pub is_program: bool,
    pub pre_balance: u64,
    pub post_balance: u64,
    pub balance_change: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceChange {
    pub account: String,
    pub before: u64,
    pub after: u64,
    pub change: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBalance {
    pub account_index: u8,
    pub mint: String,
    pub owner: Option<String>,
    pub program_id: Option<String>,
    pub amount: String,
    pub decimals: u8,
    pub ui_amount: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBalanceChange {
    pub account: String,
    pub mint: String,
    pub before: TokenAmount,
    pub after: TokenAmount,
    pub change: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAmount {
    pub amount: String,
    pub decimals: u8,
    pub ui_amount: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedInstruction {
    pub program_id: String,
    pub program_name: Option<String>,
    pub instruction_type: Option<String>,
    pub accounts: Vec<String>,
    pub data: String,
    pub parsed: Option<ParsedInstructionData>,
    pub stack_height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedInstructionData {
    pub instruction_type: String,
    pub info: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InnerInstructionSet {
    pub index: u8,
    pub instructions: Vec<ExtractedInstruction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnData {
    pub program_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressTableLookup {
    pub account_key: String,
    pub writable_indexes: Vec<u8>,
    pub readonly_indexes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedAddresses {
    pub writable: Vec<String>,
    pub readonly: Vec<String>,
}

pub struct TransactionExtractor {
    rpc_client: Arc<RpcClientWithFailover>,
}

impl TransactionExtractor {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_client: Arc::new(RpcClientWithFailover::new(rpc_url)),
        }
    }

    pub async fn extract_all_from_slots(&self, slots: Vec<u64>) -> Result<Vec<ExtractedTransaction>> {
        let mut all_transactions = Vec::new();
        
        for slot in slots {
            info!("Extracting transactions from slot {}", slot);
            match self.extract_from_slot(slot).await {
                Ok(transactions) => {
                    info!("Extracted {} transactions from slot {}", transactions.len(), slot);
                    all_transactions.extend(transactions);
                }
                Err(e) => {
                    error!("Failed to extract from slot {}: {}", slot, e);
                }
            }
        }
        
        Ok(all_transactions)
    }

    pub async fn extract_from_slot(&self, slot: u64) -> Result<Vec<ExtractedTransaction>> {
        let block = self.rpc_client
            .get_block_with_config(
                slot,
                solana_client::rpc_config::RpcBlockConfig {
                    encoding: Some(UiTransactionEncoding::JsonParsed),
                    transaction_details: Some(solana_transaction_status::TransactionDetails::Full),
                    rewards: Some(false),
                    commitment: None,
                    max_supported_transaction_version: Some(0),
                },
            )
            .await
            .context(format!("Failed to fetch block for slot {}", slot))?;

        let mut extracted_transactions = Vec::new();
        
        if let Some(transactions) = block.transactions {
            for (idx, tx_with_meta) in transactions.into_iter().enumerate() {
                match self.extract_transaction(tx_with_meta, slot, block.block_time, block.block_height) {
                    Ok(extracted) => extracted_transactions.push(extracted),
                    Err(e) => {
                        warn!("Failed to extract transaction at index {}: {}", idx, e);
                    }
                }
            }
        }
        
        Ok(extracted_transactions)
    }

    fn extract_transaction(
        &self,
        tx_with_meta: EncodedTransactionWithStatusMeta,
        slot: u64,
        block_time: Option<i64>,
        block_height: Option<u64>,
    ) -> Result<ExtractedTransaction> {
        let meta = tx_with_meta.meta.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Transaction meta is missing"))?;

        let (signature, recent_blockhash, account_keys, instructions, address_table_lookups, version) = 
            self.extract_transaction_details(&tx_with_meta.transaction)?;

        // Extract account information with balance changes
        let accounts = self.extract_account_info(
            &account_keys,
            &meta.pre_balances,
            &meta.post_balances,
            &tx_with_meta.transaction,
        )?;

        // Extract balance changes
        let balance_changes = self.extract_balance_changes(
            &account_keys,
            &meta.pre_balances,
            &meta.post_balances,
        );

        // Extract token balances
        let pre_token_balances_opt = match &meta.pre_token_balances {
            OptionSerializer::Some(balances) => Some(balances.clone()),
            _ => None,
        };
        let post_token_balances_opt = match &meta.post_token_balances {
            OptionSerializer::Some(balances) => Some(balances.clone()),
            _ => None,
        };
        let pre_token_balances = self.extract_token_balances(&pre_token_balances_opt)?;
        let post_token_balances = self.extract_token_balances(&post_token_balances_opt)?;
        let token_balance_changes = self.calculate_token_balance_changes(
            &pre_token_balances,
            &post_token_balances,
            &account_keys,
        );

        // Extract instructions
        let extracted_instructions = self.extract_instructions(&instructions, &account_keys)?;
        
        // Extract inner instructions
        let inner_instructions_opt = match &meta.inner_instructions {
            OptionSerializer::Some(inner) => Some(inner.clone()),
            _ => None,
        };
        let inner_instructions = self.extract_inner_instructions(&inner_instructions_opt, &account_keys)?;

        // Extract logs
        let log_messages = match &meta.log_messages {
            OptionSerializer::Some(logs) => logs.clone(),
            _ => Vec::new(),
        };

        // Extract return data
        let return_data = match &meta.return_data {
            OptionSerializer::Some(data) => Some(ReturnData {
                program_id: data.program_id.clone(),
                data: format!("{:?}", data.data), // Convert to base64 or hex
            }),
            _ => None,
        };

        // Extract loaded addresses
        let loaded_addresses_opt = match &meta.loaded_addresses {
            OptionSerializer::Some(loaded) => Some(loaded.clone()),
            _ => None,
        };
        let loaded_addresses = self.extract_loaded_addresses(&loaded_addresses_opt);

        Ok(ExtractedTransaction {
            signature,
            slot,
            block_time,
            block_height,
            success: meta.err.is_none(),
            fee: meta.fee,
            error: meta.err.as_ref().map(|e| format!("{:?}", e)),
            compute_units_consumed: match meta.compute_units_consumed {
                OptionSerializer::Some(units) => Some(units),
                _ => None,
            },
            accounts,
            account_keys: account_keys.clone(),
            static_account_keys: account_keys.clone(), // TODO: Differentiate static vs dynamic
            writable_account_indices: vec![], // TODO: Extract from message header
            readonly_account_indices: vec![], // TODO: Extract from message header
            pre_balances: meta.pre_balances.clone(),
            post_balances: meta.post_balances.clone(),
            balance_changes,
            pre_token_balances,
            post_token_balances,
            token_balance_changes,
            instructions: extracted_instructions,
            inner_instructions,
            log_messages,
            return_data,
            address_table_lookups,
            version,
            recent_blockhash,
            loaded_addresses,
        })
    }

    fn extract_transaction_details(
        &self,
        transaction: &EncodedTransaction,
    ) -> Result<(String, String, Vec<String>, Vec<UiInstruction>, Vec<AddressTableLookup>, String)> {
        match transaction {
            EncodedTransaction::Json(ui_tx) => {
                let signature = ui_tx.signatures.first()
                    .ok_or_else(|| anyhow::anyhow!("No signature found"))?
                    .clone();
                
                let (recent_blockhash, account_keys, instructions, address_table_lookups) = match &ui_tx.message {
                    UiMessage::Parsed(parsed_msg) => {
                        let account_keys: Vec<String> = parsed_msg.account_keys.iter()
                            .map(|ak| ak.pubkey.clone())
                            .collect();
                        
                        (
                            parsed_msg.recent_blockhash.clone(),
                            account_keys,
                            parsed_msg.instructions.clone(),
                            vec![], // Address table lookups not in parsed message
                        )
                    },
                    UiMessage::Raw(raw_msg) => {
                        (
                            raw_msg.recent_blockhash.clone(),
                            raw_msg.account_keys.clone(),
                            self.convert_compiled_instructions(&raw_msg.instructions),
                            vec![], // TODO: Extract address table lookups
                        )
                    },
                };
                
                let version = "legacy".to_string(); // TODO: Extract actual version
                
                Ok((signature, recent_blockhash, account_keys, instructions, address_table_lookups, version))
            },
            _ => Err(anyhow::anyhow!("Unsupported transaction encoding")),
        }
    }

    fn convert_compiled_instructions(&self, instructions: &[UiCompiledInstruction]) -> Vec<UiInstruction> {
        instructions.iter()
            .map(|inst| UiInstruction::Compiled(inst.clone()))
            .collect()
    }

    fn extract_account_info(
        &self,
        account_keys: &[String],
        pre_balances: &[u64],
        post_balances: &[u64],
        transaction: &EncodedTransaction,
    ) -> Result<Vec<AccountInfo>> {
        let mut accounts = Vec::new();
        
        for (idx, account_key) in account_keys.iter().enumerate() {
            let pre_balance = pre_balances.get(idx).copied().unwrap_or(0);
            let post_balance = post_balances.get(idx).copied().unwrap_or(0);
            let balance_change = post_balance as i64 - pre_balance as i64;
            
            // TODO: Extract signer/writable/program info from transaction
            let account_info = AccountInfo {
                pubkey: account_key.clone(),
                is_signer: false, // TODO: Extract from message
                is_writable: false, // TODO: Extract from message
                is_program: false, // TODO: Detect program accounts
                pre_balance,
                post_balance,
                balance_change,
            };
            
            accounts.push(account_info);
        }
        
        Ok(accounts)
    }

    fn extract_balance_changes(
        &self,
        account_keys: &[String],
        pre_balances: &[u64],
        post_balances: &[u64],
    ) -> HashMap<String, BalanceChange> {
        let mut changes = HashMap::new();
        
        for (idx, account_key) in account_keys.iter().enumerate() {
            let before = pre_balances.get(idx).copied().unwrap_or(0);
            let after = post_balances.get(idx).copied().unwrap_or(0);
            let change = after as i64 - before as i64;
            
            if change != 0 {
                changes.insert(
                    account_key.clone(),
                    BalanceChange {
                        account: account_key.clone(),
                        before,
                        after,
                        change,
                    },
                );
            }
        }
        
        changes
    }

    fn extract_token_balances(
        &self,
        token_balances: &Option<Vec<UiTransactionTokenBalance>>,
    ) -> Result<Vec<TokenBalance>> {
        Ok(token_balances.as_ref()
            .map(|balances| {
                balances.iter()
                    .map(|tb| TokenBalance {
                        account_index: tb.account_index,
                        mint: tb.mint.clone(),
                        owner: tb.owner.clone().map(|o| o.to_string()),
                        program_id: tb.program_id.clone().map(|p| p.to_string()),
                        amount: tb.ui_token_amount.amount.clone(),
                        decimals: tb.ui_token_amount.decimals,
                        ui_amount: tb.ui_token_amount.ui_amount,
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn calculate_token_balance_changes(
        &self,
        pre_balances: &[TokenBalance],
        post_balances: &[TokenBalance],
        account_keys: &[String],
    ) -> Vec<TokenBalanceChange> {
        let mut changes = Vec::new();
        
        // Create maps for easier lookup
        let pre_map: HashMap<(u8, &str), &TokenBalance> = pre_balances.iter()
            .map(|tb| ((tb.account_index, tb.mint.as_str()), tb))
            .collect();
            
        let post_map: HashMap<(u8, &str), &TokenBalance> = post_balances.iter()
            .map(|tb| ((tb.account_index, tb.mint.as_str()), tb))
            .collect();
        
        // Check all unique (account_index, mint) pairs
        let mut all_keys = pre_map.keys().chain(post_map.keys()).collect::<std::collections::HashSet<_>>();
        
        for &&(account_index, mint) in all_keys.iter() {
            let pre_balance = pre_map.get(&(account_index, mint));
            let post_balance = post_map.get(&(account_index, mint));
            
            if let Some(account) = account_keys.get(account_index as usize) {
                let before = pre_balance.map(|tb| TokenAmount {
                    amount: tb.amount.clone(),
                    decimals: tb.decimals,
                    ui_amount: tb.ui_amount,
                }).unwrap_or(TokenAmount {
                    amount: "0".to_string(),
                    decimals: 0,
                    ui_amount: Some(0.0),
                });
                
                let after = post_balance.map(|tb| TokenAmount {
                    amount: tb.amount.clone(),
                    decimals: tb.decimals,
                    ui_amount: tb.ui_amount,
                }).unwrap_or(TokenAmount {
                    amount: "0".to_string(),
                    decimals: 0,
                    ui_amount: Some(0.0),
                });
                
                let change = after.ui_amount.unwrap_or(0.0) - before.ui_amount.unwrap_or(0.0);
                
                if change.abs() > 0.0 {
                    changes.push(TokenBalanceChange {
                        account: account.clone(),
                        mint: mint.to_string(),
                        before,
                        after,
                        change,
                    });
                }
            }
        }
        
        changes
    }

    fn extract_instructions(
        &self,
        instructions: &[UiInstruction],
        account_keys: &[String],
    ) -> Result<Vec<ExtractedInstruction>> {
        instructions.iter()
            .map(|inst| self.extract_single_instruction(inst, account_keys))
            .collect()
    }

    fn extract_single_instruction(
        &self,
        instruction: &UiInstruction,
        account_keys: &[String],
    ) -> Result<ExtractedInstruction> {
        match instruction {
            UiInstruction::Compiled(compiled) => {
                let program_id = account_keys.get(compiled.program_id_index as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("Unknown({})", compiled.program_id_index));
                
                let accounts = compiled.accounts.iter()
                    .map(|&idx| account_keys.get(idx as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("Unknown({})", idx)))
                    .collect();
                
                Ok(ExtractedInstruction {
                    program_id,
                    program_name: None,
                    instruction_type: None,
                    accounts,
                    data: compiled.data.clone(),
                    parsed: None,
                    stack_height: compiled.stack_height,
                })
            },
            UiInstruction::Parsed(parsed) => {
                match parsed {
                    UiParsedInstruction::Parsed(parsed_inst) => {
                        Ok(ExtractedInstruction {
                            program_id: parsed_inst.program_id.clone(),
                            program_name: Some(parsed_inst.program.clone()),
                            instruction_type: Some(parsed_inst.parsed.get("type")
                                .and_then(|t| t.as_str())
                                .unwrap_or("unknown")
                                .to_string()),
                            accounts: vec![], // TODO: Extract from parsed info
                            data: serde_json::to_string(&parsed_inst.parsed).unwrap_or_default(),
                            parsed: Some(ParsedInstructionData {
                                instruction_type: parsed_inst.parsed.get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown")
                                    .to_string(),
                                info: parsed_inst.parsed.get("info")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            }),
                            stack_height: None,
                        })
                    },
                    UiParsedInstruction::PartiallyDecoded(partial) => {
                        let accounts = partial.accounts.iter()
                            .map(|acc| acc.to_string())
                            .collect();
                        
                        Ok(ExtractedInstruction {
                            program_id: partial.program_id.clone(),
                            program_name: None,
                            instruction_type: None,
                            accounts,
                            data: partial.data.clone(),
                            parsed: None,
                            stack_height: partial.stack_height,
                        })
                    },
                }
            },
        }
    }

    fn extract_inner_instructions(
        &self,
        inner_instructions: &Option<Vec<solana_transaction_status::UiInnerInstructions>>,
        account_keys: &[String],
    ) -> Result<Vec<InnerInstructionSet>> {
        Ok(inner_instructions.as_ref()
            .map(|inner_sets| {
                inner_sets.iter()
                    .map(|set| InnerInstructionSet {
                        index: set.index,
                        instructions: set.instructions.iter()
                            .filter_map(|inst| self.extract_single_instruction(inst, account_keys).ok())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn extract_loaded_addresses(
        &self,
        loaded_addresses: &Option<solana_transaction_status::UiLoadedAddresses>,
    ) -> LoadedAddresses {
        loaded_addresses.as_ref()
            .map(|la| LoadedAddresses {
                writable: la.writable.clone(),
                readonly: la.readonly.clone(),
            })
            .unwrap_or(LoadedAddresses {
                writable: vec![],
                readonly: vec![],
            })
    }
}

/// Create a JSON export of all extracted transactions
pub fn export_transactions_to_json(
    transactions: &[ExtractedTransaction],
    output_path: &str,
) -> Result<()> {
    let json = serde_json::to_string_pretty(transactions)?;
    std::fs::write(output_path, json)?;
    info!("Exported {} transactions to {}", transactions.len(), output_path);
    Ok(())
}

/// Create a CSV export of transaction summaries
pub fn export_transaction_summary_csv(
    transactions: &[ExtractedTransaction],
    output_path: &str,
) -> Result<()> {
    use std::io::Write;
    
    let mut file = std::fs::File::create(output_path)?;
    writeln!(file, "signature,slot,timestamp,success,fee,compute_units,num_instructions,num_logs,total_balance_change")?;
    
    for tx in transactions {
        let total_balance_change: i64 = tx.balance_changes.values()
            .map(|bc| bc.change.abs())
            .sum();
        
        writeln!(
            file,
            "{},{},{},{},{},{},{},{},{}",
            tx.signature,
            tx.slot,
            tx.block_time.unwrap_or(0),
            tx.success,
            tx.fee,
            tx.compute_units_consumed.unwrap_or(0),
            tx.instructions.len(),
            tx.log_messages.len(),
            total_balance_change,
        )?;
    }
    
    info!("Exported transaction summary to {}", output_path);
    Ok(())
}