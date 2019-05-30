extern crate core;
extern crate serde;
extern crate zei;
#[macro_use]
extern crate serde_derive;

use core::data_model::errors::PlatformError;
use core::data_model::{
  AccountAddress, AssetCreation, AssetCreationBody, AssetIssuance, AssetIssuanceBody,
  AssetTokenCode, AssetTransfer, AssetTransferBody, ConfidentialMemo, IssuerPublicKey, Memo,
  Operation, Transaction, TxOutput, TxoSID,
};
use rand::SeedableRng;
use rand_chacha::ChaChaRng;
use zei::basic_crypto::signatures::XfrSecretKey;
use zei::transfers::{open_asset_record, AssetRecord, BlindAssetRecord, OpenAssetRecord};

pub trait BuildsTransactions {
  fn transaction(&self) -> &Transaction;
  fn add_operation_create_asset(&mut self,
                                pub_key: &IssuerPublicKey,
                                priv_key: &XfrSecretKey,
                                token_code: Option<AssetTokenCode>,
                                updatable: bool,
                                memo: &str,
                                make_confidential: bool)
                                -> Result<(), PlatformError>;
  fn add_operation_issue_asset(&mut self,
                               pub_key: &IssuerPublicKey,
                               priv_key: &XfrSecretKey,
                               token_code: &AssetTokenCode,
                               seq_num: u64,
                               records: &[TxOutput])
                               -> Result<(), PlatformError>;
  fn add_operation_transfer_asset(&mut self,
                                  input_sids: Vec<TxoSID>,
                                  input_records: &[OpenAssetRecord],
                                  output_records: &[AssetRecord])
                                  -> Result<(), PlatformError>;
  fn serialize(&self) -> Result<Vec<u8>, PlatformError>;

  fn add_basic_transfer_asset(&mut self,
                              transfer_from: &[(&TxoSID,
                                 &BlindAssetRecord,
                                 u64,
                                 &AccountAddress,
                                 &XfrSecretKey)],
                              transfer_to: &[(u64, &AccountAddress)])
                              -> Result<(), PlatformError> {
    let input_sids: Vec<TxoSID> = transfer_from.iter()
                                               .map(|(ref txo_sid, _, _, _, _)| *(*txo_sid))
                                               .collect();
    let input_amounts: Vec<u64> = transfer_from.iter()
                                               .map(|(_, _, amount, _, _)| *amount)
                                               .collect();
    let input_oars: Result<Vec<OpenAssetRecord>, _> =
      transfer_from.iter()
                   .map(|(_, ref ba, _, _, ref sk)| {
                     open_asset_record(&ba, &sk).or(Err(PlatformError::ZeiError))
                   })
                   .collect();
    let input_oars = input_oars?;
    let input_total: u64 = input_amounts.iter().sum();
    let mut partially_consumed_inputs = Vec::new();
    for (input_amount, oar) in input_amounts.iter().zip(input_oars.iter()) {
      if input_amount > oar.get_amount() {
        return Err(PlatformError::InputsError);
      } else if input_amount < oar.get_amount() {
        let ar = AssetRecord::new(oar.get_amount() - input_amount,
                                  *oar.get_asset_type(),
                                  *oar.get_pub_key()).or(Err(PlatformError::ZeiError))?;
        partially_consumed_inputs.push(ar);
      }
    }
    let output_total = transfer_to.iter().fold(0, |acc, (amount, _)| acc + amount);
    if input_total != output_total {
      return Err(PlatformError::InputsError);
    }
    let asset_type = input_oars[0].get_asset_type();
    let output_ars: Result<Vec<AssetRecord>, _> =
      transfer_to.iter()
                 .map(|(amount, ref addr)| {
                   AssetRecord::new(*amount, *asset_type, addr.key).or(Err(PlatformError::ZeiError))
                 })
                 .collect();
    let mut output_ars = output_ars?;
    output_ars.append(&mut partially_consumed_inputs);
    self.add_operation_transfer_asset(input_sids, &input_oars, &output_ars)
  }
}

#[derive(Default, Serialize, Deserialize)]
pub struct TransactionBuilder {
  txn: Transaction,
  outputs: u64,
}

impl BuildsTransactions for TransactionBuilder {
  fn transaction(&self) -> &Transaction {
    &self.txn
  }
  fn add_operation_create_asset(&mut self,
                                pub_key: &IssuerPublicKey,
                                priv_key: &XfrSecretKey,
                                token_code: Option<AssetTokenCode>,
                                updatable: bool,
                                _memo: &str,
                                make_confidential: bool)
                                -> Result<(), PlatformError> {
    let memo;
    let confidential_memo = if make_confidential {
      memo = None;
      Some(ConfidentialMemo {})
    } else {
      memo = Some(Memo {});
      None
    };

    self.txn.add_operation(Operation::AssetCreation(AssetCreation::new(AssetCreationBody::new(&token_code.unwrap_or_else(AssetTokenCode::gen_random), pub_key, updatable, memo, confidential_memo)?, pub_key, priv_key)?));
    Ok(())
  }
  fn add_operation_issue_asset(&mut self,
                               pub_key: &IssuerPublicKey,
                               priv_key: &XfrSecretKey,
                               token_code: &AssetTokenCode,
                               seq_num: u64,
                               records: &[TxOutput])
                               -> Result<(), PlatformError> {
    let mut outputs = self.txn.outputs;
    self.txn.add_operation(Operation::AssetIssuance(AssetIssuance::new(AssetIssuanceBody::new(token_code, seq_num, records, &mut outputs)?, pub_key, priv_key)?));
    self.txn.outputs = outputs;
    Ok(())
  }
  fn add_operation_transfer_asset(&mut self,
                                  input_sids: Vec<TxoSID>,
                                  input_records: &[OpenAssetRecord],
                                  output_records: &[AssetRecord])
                                  -> Result<(), PlatformError> {
    let mut prng: ChaChaRng;
    prng = ChaChaRng::from_seed([0u8; 32]);
    let input_keys = Vec::new(); // TODO: multisig support...
    let mut outputs = self.txn.outputs;
    self.txn.add_operation(Operation::AssetTransfer(AssetTransfer::new(AssetTransferBody::new(&mut prng, input_sids, input_records, output_records, &input_keys, &mut outputs)?)?));
    self.txn.outputs = outputs;
    Ok(())
  }
  fn serialize(&self) -> Result<Vec<u8>, PlatformError> {
    let j = serde_json::to_string(&self.txn)?;
    Ok(j.as_bytes().to_vec())
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn it_works() {
    assert_eq!(2 + 2, 4);
  }
}
