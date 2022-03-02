//!
//! # Cached data for APIs
//!

use {
    crate::{
        data_model::{
            ATxoSID, AXfrAddress, AssetTypeCode, DefineAsset, IssueAsset,
            IssuerPublicKey, Operation, Transaction, TxOutput, TxnIDHash, TxnSID,
            TxoSID, XfrAddress,
        },
        staking::{
            ops::mint_fra::MintEntry, Amount, BlockHeight, DelegationRwdDetail,
            CHAN_D_AMOUNT_HIST, CHAN_D_RWD_HIST, CHAN_GLOB_RATE_HIST,
            CHAN_V_SELF_D_HIST, KEEP_HIST,
        },
        store::LedgerState,
    },
    fbnc::{new_mapx, new_mapxnk, Mapx, Mapxnk},
    globutils::wallet,
    ruc::*,
    serde::{Deserialize, Serialize},
    std::collections::HashSet,
    zei::xfr::{sig::XfrPublicKey, structs::OwnerMemo},
};

type Issuances = Vec<(TxOutput, Option<OwnerMemo>)>;

/// Used in APIs
#[derive(Clone, Deserialize, Serialize)]
pub struct ApiCache {
    pub(crate) prefix: String,
    /// Set of transactions related to a ledger address
    pub related_transactions: Mapx<XfrAddress, Mapxnk<TxnSID, bool>>,
    /// Set of transfer transactions related to an asset code
    pub related_transfers: Mapx<AssetTypeCode, Mapxnk<TxnSID, bool>>,
    /// List of claim transactions related to a ledger address
    pub claim_hist_txns: Mapx<XfrAddress, Mapxnk<TxnSID, bool>>,
    /// Payments from coinbase
    pub coinbase_oper_hist: Mapx<XfrAddress, Mapxnk<BlockHeight, MintEntry>>,
    /// Created assets
    pub created_assets: Mapx<IssuerPublicKey, Mapxnk<AssetTypeCode, DefineAsset>>,
    /// issuance mapped by public key
    pub issuances: Mapx<IssuerPublicKey, Issuances>,
    /// issuance mapped by token code
    pub token_code_issuances: Mapx<AssetTypeCode, Issuances>,
    /// used in confidential tx
    pub owner_memos: Mapxnk<TxoSID, OwnerMemo>,
    /// used in anonymous tx
    pub abar_memos: Mapx<ATxoSID, OwnerMemo>,
    /// ownship of txo
    pub utxos_to_map_index: Mapxnk<TxoSID, XfrAddress>,
    /// ownship of atxo
    pub atxos_to_map_index: Mapx<ATxoSID, AXfrAddress>,
    /// txo(spent, unspent) to authenticated txn (sid, hash)
    pub txo_to_txnid: Mapxnk<TxoSID, TxnIDHash>,
    /// atxo to authenticated txn (sid, hash)
    pub atxo_to_txnid: Mapx<ATxoSID, TxnIDHash>,
    /// txn sid to txn hash
    pub txn_sid_to_hash: Mapxnk<TxnSID, String>,
    /// txn hash to txn sid
    pub txn_hash_to_sid: Mapx<String, TxnSID>,
    /// global rate history
    pub staking_global_rate_hist: Mapxnk<BlockHeight, [u128; 2]>,
    /// - self-delegation amount history
    ///   - `NonConfidential` FRAs amount
    ///   - only valid for validators
    pub staking_self_delegation_hist: Mapx<XfrPublicKey, Mapxnk<BlockHeight, Amount>>,
    /// - delegation amount per block height
    /// - only valid for a validator
    pub staking_delegation_amount_hist: Mapx<XfrPublicKey, Mapxnk<BlockHeight, Amount>>,
    /// rewards history, used on some pulic nodes, such as fullnode
    pub staking_delegation_rwd_hist:
        Mapx<XfrPublicKey, Mapxnk<BlockHeight, DelegationRwdDetail>>,
}

impl ApiCache {
    pub(crate) fn new(prefix: &str) -> Self {
        ApiCache {
            prefix: prefix.to_owned(),
            related_transactions: new_mapx!(format!(
                "api_cache/{}related_transactions",
                prefix
            )),
            related_transfers: new_mapx!(format!(
                "api_cache/{}related_transfers",
                prefix
            )),
            claim_hist_txns: new_mapx!(format!("api_cache/{}claim_hist_txns", prefix)),
            coinbase_oper_hist: new_mapx!(format!(
                "api_cache/{}coinbase_oper_hist",
                prefix
            )),
            created_assets: new_mapx!(format!("api_cache/{}created_assets", prefix)),
            issuances: new_mapx!(format!("api_cache/{}issuances", prefix)),
            token_code_issuances: new_mapx!(format!(
                "api_cache/{}token_code_issuances",
                prefix
            )),
            owner_memos: new_mapxnk!(format!("api_cache/{}owner_memos", prefix)),
            abar_memos: new_mapx!(format!("api_cache/{}abar_memos", prefix)),
            utxos_to_map_index: new_mapxnk!(format!(
                "api_cache/{}utxos_to_map_index",
                prefix
            )),
            atxos_to_map_index: new_mapx!(format!(
                "api_cache/{}atxos_to_map_index",
                prefix
            )),
            txo_to_txnid: new_mapxnk!(format!("api_cache/{}txo_to_txnid", prefix)),
            atxo_to_txnid: new_mapx!(format!("api_cache/{}atxo_to_txnid", prefix)),
            txn_sid_to_hash: new_mapxnk!(format!("api_cache/{}txn_sid_to_hash", prefix)),
            txn_hash_to_sid: new_mapx!(format!("api_cache/{}txn_hash_to_sid", prefix)),
            staking_global_rate_hist: new_mapxnk!(format!(
                "api_cache/{}staking_global_rate_hist",
                prefix
            )),
            staking_self_delegation_hist: new_mapx!(format!(
                "api_cache/{}staking_self_delegation_hist",
                prefix
            )),
            staking_delegation_amount_hist: new_mapx!(format!(
                "api_cache/{}staking_delegation_amount_hist",
                prefix
            )),
            staking_delegation_rwd_hist: new_mapx!(format!(
                "api_cache/{}staking_delegation_rwd_hist",
                prefix
            )),
        }
    }

    /// Add created asset
    #[inline(always)]
    pub fn add_created_asset(&mut self, creation: &DefineAsset) {
        let prefix = self.prefix.clone();
        let issuer = creation.pubkey;
        self.created_assets
            .entry(issuer)
            .or_insert_with(|| {
                new_mapxnk!(format!(
                    "api_cache/{}created_assets/{}",
                    prefix,
                    issuer.to_base64()
                ))
            })
            .insert(creation.body.asset.code, creation.clone());
    }

    /// Cache issuance records
    pub fn cache_issuance(&mut self, issuance: &IssueAsset) {
        let new_records = issuance.body.records.to_vec();

        macro_rules! save_issuance {
            ($maps: tt, $key: tt) => {
                #[allow(unused_mut)]
                let mut records = $maps.entry($key).or_insert_with(Vec::new);
                records.extend_from_slice(&new_records);
            };
        }

        let key_issuances = &mut self.issuances;
        let pubkey = issuance.pubkey;
        save_issuance!(key_issuances, pubkey);

        let token_issuances = &mut self.token_code_issuances;
        let token_code = issuance.body.code;
        save_issuance!(token_issuances, token_code);
    }

    /// Cache history style data
    pub fn cache_hist_data(&mut self) {
        CHAN_GLOB_RATE_HIST.1.lock().try_iter().for_each(|(h, r)| {
            self.staking_global_rate_hist.insert(h, r);
        });

        CHAN_V_SELF_D_HIST
            .1
            .lock()
            .try_iter()
            .for_each(|(pk, h, r)| {
                self.staking_self_delegation_hist
                    .entry(pk)
                    .or_insert(new_mapxnk!(format!(
                        "staking_self_delegation_hist_subdata/{}",
                        wallet::public_key_to_base64(&pk)
                    )))
                    .insert(h, r);
            });

        CHAN_D_AMOUNT_HIST
            .1
            .lock()
            .try_iter()
            .for_each(|(pk, h, r)| {
                self.staking_delegation_amount_hist
                    .entry(pk)
                    .or_insert(new_mapxnk!(format!(
                        "staking_delegation_amount_hist_subdata/{}",
                        wallet::public_key_to_base64(&pk)
                    )))
                    .insert(h, r);
            });

        CHAN_D_RWD_HIST.1.lock().try_iter().for_each(|(pk, h, r)| {
            #[allow(unused_mut)]
            let mut dd =
                self.staking_delegation_rwd_hist
                    .entry(pk)
                    .or_insert(new_mapxnk!(format!(
                        "staking_delegation_rwd_hist_subdata/{}",
                        wallet::public_key_to_base64(&pk)
                    )));
            let mut dd = dd.entry(h).or_insert_with(DelegationRwdDetail::default);

            dd.block_height = r.block_height;
            dd.amount += r.amount;
            dd.penalty_amount += r.penalty_amount;

            alt!(0 < r.bond, dd.bond = r.bond);
            alt!(r.return_rate.is_some(), dd.return_rate = r.return_rate);
            alt!(
                r.commission_rate.is_some(),
                dd.commission_rate = r.commission_rate
            );
            alt!(
                r.global_delegation_percent.is_some(),
                dd.global_delegation_percent = r.global_delegation_percent
            );
        });
    }
}

/// An xfr address is related to a transaction if it is one of the following:
/// 1. Owner of a transfer output
/// 2. Transfer signer (owner of input or co-signer)
/// 3. Signer of a an issuance txn
/// 4. Signer of a kv_update txn
/// 5. Signer of a memo_update txn
pub fn get_related_addresses<F>(
    txn: &Transaction,
    mut classify: F,
) -> HashSet<XfrAddress>
where
    F: FnMut(&Operation),
{
    let mut related_addresses = HashSet::new();

    macro_rules! staking_gen {
        ($op: expr) => {{
            $op.get_related_pubkeys().into_iter().for_each(|pk| {
                related_addresses.insert(XfrAddress { key: pk });
            });
        }};
    }

    for op in &txn.body.operations {
        classify(op);
        match op {
            Operation::UpdateStaker(i) => staking_gen!(i),
            Operation::Delegation(i) => staking_gen!(i),
            Operation::UnDelegation(i) => staking_gen!(i),
            Operation::Claim(i) => staking_gen!(i),
            Operation::UpdateValidator(i) => staking_gen!(i),
            Operation::Governance(i) => staking_gen!(i),
            Operation::FraDistribution(i) => staking_gen!(i),
            Operation::MintFra(i) => staking_gen!(i),
            Operation::BarToAbar(i) => {
                related_addresses.insert(XfrAddress {
                    key: i.note.body.input.public_key,
                });
            }
            Operation::AbarToBar(i) => {
                related_addresses.insert(XfrAddress {
                    key: i.note.body.output.public_key,
                });
            }
            Operation::TransferAnonAsset(_) => {
                // Anon
            }
            Operation::ConvertAccount(i) => {
                related_addresses.insert(XfrAddress {
                    key: i.get_related_address(),
                });
            }
            Operation::TransferAsset(transfer) => {
                for input in transfer.body.transfer.inputs.iter() {
                    related_addresses.insert(XfrAddress {
                        key: input.public_key,
                    });
                }

                for output in transfer.body.transfer.outputs.iter() {
                    related_addresses.insert(XfrAddress {
                        key: output.public_key,
                    });
                }
            }
            Operation::IssueAsset(issue_asset) => {
                related_addresses.insert(XfrAddress {
                    key: issue_asset.pubkey.key,
                });
            }
            Operation::DefineAsset(define_asset) => {
                related_addresses.insert(XfrAddress {
                    key: define_asset.pubkey.key,
                });
            }
            Operation::UpdateMemo(update_memo) => {
                related_addresses.insert(XfrAddress {
                    key: update_memo.pubkey,
                });
            }
        }
    }
    related_addresses
}

/// Returns the set of nonconfidential assets transferred in a transaction.
pub fn get_transferred_nonconfidential_assets(
    txn: &Transaction,
) -> HashSet<AssetTypeCode> {
    let mut transferred_assets = HashSet::new();
    for op in &txn.body.operations {
        if let Operation::TransferAsset(transfer) = op {
            for input in transfer.body.transfer.inputs.iter() {
                if let Some(asset_type) = input.asset_type.get_asset_type() {
                    transferred_assets.insert(AssetTypeCode { val: asset_type });
                }
            }
        }
    }
    transferred_assets
}

/// update the data of QueryServer when we create a new block in ABCI
pub fn update_api_cache(ledger: &mut LedgerState) -> Result<()> {
    if !*KEEP_HIST {
        return Ok(());
    }

    ledger.api_cache.as_mut().unwrap().cache_hist_data();

    let block = if let Some(b) = ledger.blocks.last() {
        b
    } else {
        return Ok(());
    };

    let prefix = ledger.api_cache.as_mut().unwrap().prefix.clone();

    // Update ownership status
    for (txn_sid, txo_sids, atxo_sids) in block
        .txns
        .iter()
        .map(|v| (v.tx_id, v.txo_ids.as_slice(), v.atxo_ids.as_slice()))
    {
        let curr_txn = ledger.get_transaction_light(txn_sid).c(d!())?.txn;
        // get the transaction, ownership addresses, and memos associated with each transaction
        let (addresses, owner_memos) = {
            let addresses: Vec<XfrAddress> = txo_sids
                .iter()
                .map(|sid| XfrAddress {
                    key: ((ledger
                        .get_utxo_light(*sid)
                        .or_else(|| ledger.get_spent_utxo_light(*sid))
                        .unwrap()
                        .utxo)
                        .0)
                        .record
                        .public_key,
                })
                .collect();

            let owner_memos = curr_txn.get_owner_memos_ref();

            (addresses, owner_memos)
        };

        let classify_op = |op: &Operation| {
            match op {
                Operation::Claim(i) => {
                    let key = XfrAddress {
                        key: i.get_claim_publickey(),
                    };
                    ledger
                        .api_cache
                        .as_mut()
                        .unwrap()
                        .claim_hist_txns
                        .entry(key)
                        .or_insert_with(|| {
                            new_mapxnk!(format!(
                                "api_cache/{}claim_hist_txns/{}",
                                prefix,
                                key.to_base64()
                            ))
                        })
                        .set_value(txn_sid, Default::default());
                }
                Operation::MintFra(i) => i.entries.iter().for_each(|me| {
                    let key = XfrAddress {
                        key: me.utxo.record.public_key,
                    };
                    #[allow(unused_mut)]
                    let mut hist = ledger
                        .api_cache
                        .as_mut()
                        .unwrap()
                        .coinbase_oper_hist
                        .entry(key)
                        .or_insert_with(|| {
                            new_mapxnk!(format!(
                                "api_cache/{}coinbase_oper_hist/{}",
                                prefix,
                                key.to_base64()
                            ))
                        });
                    hist.insert(i.height, me.clone());
                }),
                _ => { /* filter more operations before this line */ }
            };
        };

        // Update related addresses
        // Apply classify_op for each operation in curr_txn
        let related_addresses = get_related_addresses(&curr_txn, classify_op);
        for address in &related_addresses {
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .related_transactions
                .entry(*address)
                .or_insert_with(|| {
                    new_mapxnk!(format!(
                        "api_cache/{}related_transactions/{}",
                        prefix,
                        address.to_base64()
                    ))
                })
                .insert(txn_sid, Default::default());
        }

        // Update transferred nonconfidential assets
        let transferred_assets = get_transferred_nonconfidential_assets(&curr_txn);
        for asset in &transferred_assets {
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .related_transfers
                .entry(*asset)
                .or_insert_with(|| {
                    new_mapxnk!(format!(
                        "api_cache/{}related_transfers/{}",
                        &prefix,
                        asset.to_base64()
                    ))
                })
                .insert(txn_sid, Default::default());
        }

        // Add created asset
        for op in &curr_txn.body.operations {
            match op {
                Operation::DefineAsset(define_asset) => {
                    ledger
                        .api_cache
                        .as_mut()
                        .unwrap()
                        .add_created_asset(&define_asset);
                }
                Operation::IssueAsset(issue_asset) => {
                    ledger
                        .api_cache
                        .as_mut()
                        .unwrap()
                        .cache_issuance(&issue_asset);
                }
                _ => {}
            };
        }

        // Add new utxos (this handles both transfers and issuances)
        for (txo_sid, (address, owner_memo)) in txo_sids
            .iter()
            .zip(addresses.iter().zip(owner_memos.iter()))
        {
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .utxos_to_map_index
                .insert(*txo_sid, *address);
            let hash = curr_txn.hash_tm().hex().to_uppercase();
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .txo_to_txnid
                .insert(*txo_sid, (txn_sid, hash.clone()));
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .txn_sid_to_hash
                .insert(txn_sid, hash.clone());
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .txn_hash_to_sid
                .insert(hash.clone(), txn_sid);
            if let Some(owner_memo) = owner_memo {
                ledger
                    .api_cache
                    .as_mut()
                    .unwrap()
                    .owner_memos
                    .insert(*txo_sid, (*owner_memo).clone());
            }
        }

        let conv_memos = curr_txn.body.operations.iter().filter_map(|o| match o {
            Operation::BarToAbar(b) => {
                Some((b.note.body.output.public_key, b.note.body.memo.clone()))
            }
            _ => None,
        });
        let abar_memos = curr_txn
            .body
            .operations
            .iter()
            .filter_map(|o| match o {
                Operation::TransferAnonAsset(b) => Some(
                    b.note
                        .body
                        .outputs
                        .iter()
                        .zip(b.note.body.owner_memos.clone())
                        .map(|(op, memo)| (op.public_key, memo)),
                ),
                _ => None,
            })
            .flatten();

        for (a, id) in conv_memos.chain(abar_memos).zip(atxo_sids) {
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .atxos_to_map_index
                .insert(*id, AXfrAddress { key: a.0 });
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .abar_memos
                .insert(*id, a.1);
            let hash = curr_txn.hash_tm().hex().to_uppercase();
            ledger
                .api_cache
                .as_mut()
                .unwrap()
                .atxo_to_txnid
                .insert(*id, (txn_sid, hash.clone()));
        }
    }

    Ok(())
}
