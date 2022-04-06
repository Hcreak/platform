//!
//! # Findora Network Cli tool
//!
//! FN, a command line tool for findora network.
//!
//! This module is the library part of FN.
//!

pub mod evm;
pub mod utils;

use zei::xfr::structs::{XfrAmount, XfrAssetType};
use {
    crate::api::DelegationInfo,
    crate::common::utils::{new_tx_builder, send_tx},
    crate::txn_builder::TransactionBuilder,
    crypto::basics::hybrid_encryption::{XPublicKey, XSecretKey},
    globutils::wallet,
    lazy_static::lazy_static,
    ledger::{
        data_model::{
            gen_random_keypair, ATxoSID, AssetRules, AssetTypeCode, Transaction, TxoSID,
            BLACK_HOLE_PUBKEY_STAKING,
        },
        staking::{
            check_delegation_amount, td_addr_to_bytes, td_pubkey_to_td_addr,
            td_pubkey_to_td_addr_bytes, PartialUnDelegation, StakerMemo,
            TendermintAddrRef,
        },
    },
    rand_chacha::ChaChaRng,
    rand_core::SeedableRng,
    ruc::*,
    std::{env, fs},
    tendermint::PrivateKey,
    utils::{
        get_block_height, get_local_block_height, get_validator_detail,
        parse_td_validator_keys,
    },
    zei::anon_xfr::{
        anon_fee::ANON_FEE_MIN,
        keys::{AXfrKeyPair, AXfrPubKey},
        nullifier,
        structs::{AnonBlindAssetRecord, MTLeafInfo, OpenAnonBlindAssetRecordBuilder},
    },
    zei::xfr::structs::OwnerMemo,
    zei::{
        setup::PublicParams,
        xfr::{
            asset_record::AssetRecordType,
            sig::{XfrKeyPair, XfrPublicKey, XfrSecretKey},
        },
    },
    zeialgebra::{groups::Scalar, jubjub::JubjubScalar},
};

lazy_static! {
    static ref CFG_PATH: String = format!(
        "{}/.____fn_config____",
        ruc::info!(env::var("HOME")).unwrap_or_else(|_| "/tmp/".to_owned())
    );
    static ref MNEMONIC: Option<String> = fs::read_to_string(&*MNEMONIC_FILE)
        .map(|s| s.trim().to_string())
        .ok();
    static ref MNEMONIC_FILE: String = format!("{}/mnemonic", &*CFG_PATH);
    static ref TD_KEY: Option<String> = fs::read_to_string(&*TD_KEY_FILE).ok();
    static ref TD_KEY_FILE: String = format!("{}/tendermint_keys", &*CFG_PATH);
    static ref SERV_ADDR: Option<String> = fs::read_to_string(&*SERV_ADDR_FILE).ok();
    static ref SERV_ADDR_FILE: String = format!("{}/serv_addr", &*CFG_PATH);
}

/// Updating the information of a staker includes commission_rate and staker_memo
pub fn staker_update(cr: Option<&str>, memo: Option<StakerMemo>) -> Result<()> {
    let addr = get_td_pubkey().map(|i| td_pubkey_to_td_addr(&i)).c(d!())?;
    let vd = get_validator_detail(&addr).c(d!())?;

    let cr = cr
        .map_or(Ok(vd.commission_rate), |s| {
            s.parse::<f64>()
                .c(d!("commission rate must be a float number"))
                .and_then(convert_commission_rate)
        })
        .c(d!())?;
    let memo = memo.unwrap_or(vd.memo);

    let td_pubkey = get_td_pubkey().c(d!())?;

    let kp = get_keypair().c(d!())?;
    let vkp = get_td_privkey().c(d!())?;

    let mut builder = utils::new_tx_builder().c(d!())?;

    builder
        .add_operation_update_staker(&kp, &vkp, td_pubkey, cr, memo)
        .c(d!())?;
    utils::gen_fee_op(&kp)
        .c(d!())
        .map(|op| builder.add_operation(op))?;

    utils::send_tx(&builder.take_transaction()).c(d!())
}

/// Perform a staking operation to add current tendermint node to validator list
/// The cli tool user will be alert if the block height of local node is too small
pub fn stake(
    amount: &str,
    commission_rate: &str,
    memo: Option<&str>,
    force: bool,
) -> Result<()> {
    let am = amount.parse::<u64>().c(d!("'amount' must be an integer"))?;
    check_delegation_amount(am, false).c(d!())?;
    let cr = commission_rate
        .parse::<f64>()
        .c(d!("commission rate must be a float number"))
        .and_then(|cr| convert_commission_rate(cr).c(d!()))?;
    let td_pubkey = get_td_pubkey().c(d!())?;

    let kp = get_keypair().c(d!())?;
    let vkp = get_td_privkey().c(d!())?;

    macro_rules! diff {
        ($l:expr, $r:expr) => {
            if $l > $r {
                $l - $r
            } else {
                $r - $l
            }
        };
    }

    let network_height = get_block_height(get_serv_addr().unwrap());
    let local_height = get_local_block_height();
    if (network_height == 0 || local_height == 0)
        || diff!(network_height, local_height) > 3
    {
        println!(
            "The difference in block height of your node and the remote network is too big: \n remote / local: {} / {}",
            network_height, local_height
        );
        if !force {
            println!("Append option --force to ignore this warning.");
            return Ok(());
        }
        println!("Continue to stake now...");
    }

    let mut builder = utils::new_tx_builder().c(d!())?;
    builder
        .add_operation_staking(&kp, am, &vkp, td_pubkey, cr, memo.map(|m| m.to_owned()))
        .c(d!())?;
    utils::gen_transfer_op(
        &kp,
        vec![(&BLACK_HOLE_PUBKEY_STAKING, am)],
        None,
        false,
        false,
        Some(AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType),
    )
    .c(d!())
    .map(|principal_op| builder.add_operation(principal_op))?;

    utils::send_tx(&builder.take_transaction()).c(d!())
}

/// Append more FRA token to the specified tendermint node
pub fn stake_append(
    amount: &str,
    staker: Option<&str>,
    td_addr: Option<TendermintAddrRef>,
) -> Result<()> {
    let am = amount.parse::<u64>().c(d!("'amount' must be an integer"))?;
    check_delegation_amount(am, true).c(d!())?;

    let td_addr = td_addr.map(|ta| ta.to_owned()).c(d!()).or_else(|_| {
        get_td_pubkey()
            .c(d!())
            .map(|td_pk| td_pubkey_to_td_addr(&td_pk))
    })?;

    let kp = staker
        .c(d!())
        .and_then(|sk| wallet::restore_keypair_from_mnemonic_default(sk).c(d!()))
        .or_else(|_| get_keypair().c(d!()))?;

    let mut builder = utils::new_tx_builder().c(d!())?;
    builder.add_operation_delegation(&kp, am, td_addr);
    utils::gen_transfer_op(
        &kp,
        vec![(&BLACK_HOLE_PUBKEY_STAKING, am)],
        None,
        false,
        false,
        Some(AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType),
    )
    .c(d!())
    .map(|principal_op| builder.add_operation(principal_op))?;

    utils::send_tx(&builder.take_transaction()).c(d!())
}

/// Withdraw Fra token from findora network for a staker
pub fn unstake(
    am: Option<&str>,
    staker: Option<&str>,
    td_addr: Option<TendermintAddrRef>,
) -> Result<()> {
    let am = if let Some(i) = am {
        Some(i.parse::<u64>().c(d!("'amount' must be an integer"))?)
    } else {
        None
    };

    let kp = staker
        .c(d!())
        .and_then(|sk| wallet::restore_keypair_from_mnemonic_default(sk).c(d!()))
        .or_else(|_| get_keypair().c(d!()))?;
    let td_addr_bytes = td_addr
        .c(d!())
        .and_then(|ta| td_addr_to_bytes(ta).c(d!()))
        .or_else(|_| {
            get_td_pubkey()
                .c(d!())
                .map(|td_pk| td_pubkey_to_td_addr_bytes(&td_pk))
        })?;

    let mut builder = utils::new_tx_builder().c(d!())?;

    utils::gen_fee_op(&kp).c(d!()).map(|op| {
        builder.add_operation(op);
        if let Some(am) = am {
            // partial undelegation
            builder.add_operation_undelegation(
                &kp,
                Some(PartialUnDelegation::new(
                    am,
                    gen_random_keypair().get_pk(),
                    td_addr_bytes,
                )),
            );
        } else {
            builder.add_operation_undelegation(&kp, None);
        }
    })?;

    utils::send_tx(&builder.take_transaction()).c(d!())
}

/// Claim rewards from findora network
pub fn claim(am: Option<&str>, sk_str: Option<&str>) -> Result<()> {
    let am = if let Some(i) = am {
        Some(i.parse::<u64>().c(d!("'amount' must be an integer"))?)
    } else {
        None
    };

    let kp = restore_keypair_from_str_with_default(sk_str)?;

    let mut builder = utils::new_tx_builder().c(d!())?;

    utils::gen_fee_op(&kp).c(d!()).map(|op| {
        builder.add_operation(op);
        builder.add_operation_claim(&kp, am);
    })?;

    utils::send_tx(&builder.take_transaction()).c(d!())
}

/// Show information of current node, including following sections:
///     Server URL
///     Findora Wallet Address
///     Findora Public Key
///     Local validator address
///     FRA balance
///     Delegation Information
///     Validator Detail (if already staked)
///
pub fn show(basic: bool) -> Result<()> {
    let kp = get_keypair().c(d!())?;

    let serv_addr = ruc::info!(get_serv_addr()).map(|i| {
        println!("\x1b[31;01mServer URL:\x1b[00m\n{}\n", i);
    });

    let xfr_account = ruc::info!(get_keypair()).map(|i| {
        println!(
            "\x1b[31;01mFindora Address:\x1b[00m\n{}\n",
            wallet::public_key_to_bech32(&i.get_pk())
        );
        println!(
            "\x1b[31;01mFindora Public Key:\x1b[00m\n{}\n",
            wallet::public_key_to_base64(&i.get_pk())
        );
        println!(
            "\x1b[31;01mFindora Public Key in hex:\x1b[00m\n{}\n",
            wallet::public_key_to_hex(&i.get_pk())
        );
    });

    let self_balance = ruc::info!(utils::get_balance(&kp)).map(|i| {
        println!("\x1b[31;01mNode Balance:\x1b[00m\n{} FRA units\n", i);
    });

    if basic {
        return Ok(());
    }

    let td_info = ruc::info!(get_td_pubkey()).map(|i| {
        let addr = td_pubkey_to_td_addr(&i);
        println!("\x1b[31;01mValidator Node Addr:\x1b[00m\n{}\n", addr);
        (i, addr)
    });

    let di = utils::get_delegation_info(kp.get_pk_ref());
    let bond_entries = match di.as_ref() {
        Ok(di) => Some(di.bond_entries.clone()),
        Err(_) => None,
    };

    let delegation_info = di.and_then(|di| {
        serde_json::to_string_pretty(&di).c(d!("server returned invalid data"))
    });
    let delegation_info = ruc::info!(delegation_info).map(|i| {
        println!("\x1b[31;01mYour Delegation:\x1b[00m\n{}\n", i);
    });

    if let Ok((tpk, addr)) = td_info.as_ref() {
        let self_delegation =
            bond_entries.map_or(false, |bes| bes.iter().any(|i| &i.0 == addr));
        if self_delegation {
            let res = utils::get_validator_detail(&td_pubkey_to_td_addr(tpk))
                .c(d!("Validator not found"))
                .and_then(|di| {
                    serde_json::to_string_pretty(&di)
                        .c(d!("server returned invalid data"))
                })
                .map(|i| {
                    println!("\x1b[31;01mYour Staking:\x1b[00m\n{}\n", i);
                });
            ruc::info_omit!(res);
        }
    }

    if [
        serv_addr,
        xfr_account,
        td_info.map(|_| ()),
        self_balance,
        delegation_info,
    ]
    .iter()
    .any(|i| i.is_err())
    {
        Err(eg!("unable to obtain complete information"))
    } else {
        Ok(())
    }
}

/// Setup for a cli tool
///    Server URL
///    Owner mnemonic path
///    Tendermint node private key path
pub fn setup(
    serv_addr: Option<&str>,
    owner_mnemonic_path: Option<&str>,
    validator_key_path: Option<&str>,
) -> Result<()> {
    fs::create_dir_all(&*CFG_PATH).c(d!("fail to create config path"))?;

    let mut pwd = ruc::info!(
        env::current_dir(),
        "Cannot abtain current work directory, default to relative path"
    )
    .unwrap_or_default();

    if let Some(sa) = serv_addr {
        fs::write(&*SERV_ADDR_FILE, sa).c(d!("fail to cache 'serv-addr'"))?;
    }
    if let Some(mp) = owner_mnemonic_path {
        let mp = if mp.starts_with('/') {
            mp
        } else {
            pwd.push(mp);
            pwd.to_str().c(d!("Invalid path"))?
        };
        fs::write(&*MNEMONIC_FILE, mp).c(d!("fail to cache 'owner-mnemonic-path'"))?;
    }
    if let Some(kp) = validator_key_path {
        let kp = if kp.starts_with('/') {
            kp
        } else {
            pwd.push(kp);
            pwd.to_str().c(d!("Invalid path"))?
        };
        fs::write(&*TD_KEY_FILE, kp).c(d!("fail to cache 'validator-key-path'"))?;
    }
    Ok(())
}

#[allow(missing_docs)]
pub fn transfer_asset(
    owner_sk: Option<&str>,
    target_addr: XfrPublicKey,
    token_code: Option<AssetTypeCode>,
    am: &str,
    confidential_am: bool,
    confidential_ty: bool,
) -> Result<()> {
    transfer_asset_batch(
        owner_sk,
        &[target_addr],
        token_code,
        am,
        confidential_am,
        confidential_ty,
    )
    .c(d!())
}

#[allow(missing_docs)]
pub fn transfer_asset_x(
    kp: &XfrKeyPair,
    target_addr: XfrPublicKey,
    token_code: Option<AssetTypeCode>,
    am: u64,
    confidential_am: bool,
    confidential_ty: bool,
) -> Result<()> {
    transfer_asset_batch_x(
        kp,
        &[target_addr],
        token_code,
        am,
        confidential_am,
        confidential_ty,
    )
    .c(d!())
}

#[allow(missing_docs)]
pub fn transfer_asset_batch(
    owner_sk: Option<&str>,
    target_addr: &[XfrPublicKey],
    token_code: Option<AssetTypeCode>,
    am: &str,
    confidential_am: bool,
    confidential_ty: bool,
) -> Result<()> {
    let from = restore_keypair_from_str_with_default(owner_sk)?;
    let am = am.parse::<u64>().c(d!("'amount' must be an integer"))?;

    transfer_asset_batch_x(
        &from,
        target_addr,
        token_code,
        am,
        confidential_am,
        confidential_ty,
    )
    .c(d!())
}

#[allow(missing_docs)]
pub fn transfer_asset_batch_x(
    kp: &XfrKeyPair,
    target_addr: &[XfrPublicKey],
    token_code: Option<AssetTypeCode>,
    am: u64,
    confidential_am: bool,
    confidential_ty: bool,
) -> Result<()> {
    utils::transfer_batch(
        kp,
        target_addr.iter().map(|addr| (addr, am)).collect(),
        token_code,
        confidential_am,
        confidential_ty,
    )
    .c(d!())
}

/// Mainly for official usage,
/// and can be also used in test scenes.
pub fn set_initial_validators() -> Result<()> {
    utils::set_initial_validators().c(d!())
}

/// Get the effective address of server
pub fn get_serv_addr() -> Result<&'static str> {
    if let Some(sa) = SERV_ADDR.as_ref() {
        Ok(sa)
    } else {
        Err(eg!("'serv-addr' has not been set"))
    }
}

/// Get keypair from config file
pub fn get_keypair() -> Result<XfrKeyPair> {
    if let Some(m_path) = MNEMONIC.as_ref() {
        fs::read_to_string(m_path)
            .c(d!("can not read mnemonic from 'owner-mnemonic-path'"))
            .and_then(|m| {
                let k = m.trim();
                wallet::restore_keypair_from_mnemonic_default(k)
                    .c(d!("invalid 'owner-mnemonic'"))
                    .or_else(|e| wallet::restore_keypair_from_seckey_base64(k).c(d!(e)))
            })
    } else {
        Err(eg!("'owner-mnemonic-path' has not been set"))
    }
}

fn get_td_pubkey() -> Result<Vec<u8>> {
    if let Some(key_path) = TD_KEY.as_ref() {
        fs::read_to_string(key_path)
            .c(d!("can not read key file from path"))
            .and_then(|k| {
                let v_keys = parse_td_validator_keys(&k).c(d!())?;
                Ok(v_keys.pub_key.to_vec())
            })
    } else {
        Err(eg!("'validator-pubkey' has not been set"))
    }
}

fn get_td_privkey() -> Result<PrivateKey> {
    if let Some(key_path) = TD_KEY.as_ref() {
        fs::read_to_string(key_path)
            .c(d!("can not read key file from path"))
            .and_then(|k| {
                parse_td_validator_keys(&k)
                    .c(d!())
                    .map(|v_keys| v_keys.priv_key)
            })
    } else {
        Err(eg!("'validator-privkey' has not been set"))
    }
}

#[allow(missing_docs)]
pub fn convert_commission_rate(cr: f64) -> Result<[u64; 2]> {
    if 1.0 < cr {
        return Err(eg!("commission rate can exceed 100%"));
    }
    if 0.0 > cr {
        return Err(eg!("commission rate must be a positive float number"));
    }
    Ok([(cr * 10000.0) as u64, 10000])
}

#[allow(missing_docs)]
pub fn gen_key_and_print() {
    let (m, k, kp) = loop {
        let mnemonic = pnk!(wallet::generate_mnemonic_custom(24, "en"));
        let kp = pnk!(wallet::restore_keypair_from_mnemonic_default(&mnemonic));
        if let Some(key) = serde_json::to_string_pretty(&kp)
            .ok()
            .filter(|s| s.matches("\": \"-").next().is_none())
        {
            break (mnemonic, key, kp);
        }
    };
    let wallet_addr = wallet::public_key_to_bech32(kp.get_pk_ref());
    println!(
        "\n\x1b[31;01mWallet Address:\x1b[00m {}\n\x1b[31;01mMnemonic:\x1b[00m {}\n\x1b[31;01mKey:\x1b[00m {}\n",
        wallet_addr, m, k
    );
}

fn restore_keypair_from_str_with_default(sk_str: Option<&str>) -> Result<XfrKeyPair> {
    if let Some(sk) = sk_str {
        serde_json::from_str::<XfrSecretKey>(&format!("\"{}\"", sk))
            .map(|sk| sk.into_keypair())
            .c(d!("Invalid secret key"))
    } else {
        get_keypair().c(d!())
    }
}

/// Show the asset balance of a findora account
pub fn show_account(sk_str: Option<&str>, asset: Option<&str>) -> Result<()> {
    let kp = restore_keypair_from_str_with_default(sk_str)?;
    let token_code = asset
        .map(|asset| AssetTypeCode::new_from_base64(asset).c(d!("Invalid asset code")))
        .transpose()?;
    let balance = utils::get_asset_balance(&kp, token_code).c(d!())?;

    println!("{}: {}", asset.unwrap_or("FRA"), balance);
    Ok(())
}

#[inline(always)]
#[allow(missing_docs)]
pub fn delegate(sk_str: Option<&str>, amount: u64, validator: &str) -> Result<()> {
    restore_keypair_from_str_with_default(sk_str)
        .c(d!())
        .and_then(|kp| delegate_x(&kp, amount, validator).c(d!()))
}

#[inline(always)]
#[allow(missing_docs)]
pub fn delegate_x(kp: &XfrKeyPair, amount: u64, validator: &str) -> Result<()> {
    gen_delegate_tx(kp, amount, validator)
        .c(d!())
        .and_then(|tx| utils::send_tx(&tx).c(d!()))
}

#[inline(always)]
#[allow(missing_docs)]
pub fn undelegate(sk_str: Option<&str>, param: Option<(u64, &str)>) -> Result<()> {
    restore_keypair_from_str_with_default(sk_str)
        .c(d!())
        .and_then(|kp| undelegate_x(&kp, param).c(d!()))
}

#[inline(always)]
#[allow(missing_docs)]
pub fn undelegate_x(kp: &XfrKeyPair, param: Option<(u64, &str)>) -> Result<()> {
    gen_undelegate_tx(kp, param)
        .c(d!())
        .and_then(|tx| utils::send_tx(&tx).c(d!()))
}

/// Display delegation information of a findora account
pub fn show_delegations(sk_str: Option<&str>) -> Result<()> {
    let pk = restore_keypair_from_str_with_default(sk_str)?.get_pk();

    println!(
        "{}",
        serde_json::to_string_pretty::<DelegationInfo>(
            &utils::get_delegation_info(&pk).c(d!())?
        )
        .c(d!())?
    );

    Ok(())
}

fn gen_undelegate_tx(
    owner_kp: &XfrKeyPair,
    param: Option<(u64, &str)>,
) -> Result<Transaction> {
    let mut builder = utils::new_tx_builder().c(d!())?;
    utils::gen_fee_op(owner_kp).c(d!()).map(|op| {
        builder.add_operation(op);
    })?;
    if let Some((amount, validator)) = param {
        // partial undelegation
        builder.add_operation_undelegation(
            owner_kp,
            Some(PartialUnDelegation::new(
                amount,
                gen_random_keypair().get_pk(),
                td_addr_to_bytes(validator).c(d!())?,
            )),
        );
    } else {
        builder.add_operation_undelegation(owner_kp, None);
    }

    Ok(builder.take_transaction())
}

fn gen_delegate_tx(
    owner_kp: &XfrKeyPair,
    amount: u64,
    validator: &str,
) -> Result<Transaction> {
    let mut builder = utils::new_tx_builder().c(d!())?;

    utils::gen_transfer_op(
        owner_kp,
        vec![(&BLACK_HOLE_PUBKEY_STAKING, amount)],
        None,
        false,
        false,
        Some(AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType),
    )
    .c(d!())
    .map(|principal_op| {
        builder.add_operation(principal_op);
        builder.add_operation_delegation(owner_kp, amount, validator.to_owned());
    })?;

    Ok(builder.take_transaction())
}
/// Create a custom asset for a findora account. If no token code string provided,
/// it will generate a random new one.
pub fn create_asset(
    memo: &str,
    decimal: u8,
    max_units: Option<u64>,
    transferable: bool,
    token_code: Option<&str>,
) -> Result<()> {
    let kp = get_keypair().c(d!())?;

    let code = if token_code.is_none() {
        AssetTypeCode::gen_random()
    } else {
        AssetTypeCode::new_from_base64(token_code.unwrap())
            .c(d!("invalid asset code"))?
    };

    create_asset_x(&kp, memo, decimal, max_units, transferable, Some(code))
        .c(d!())
        .map(|_| ())
}

#[allow(missing_docs)]
pub fn create_asset_x(
    kp: &XfrKeyPair,
    memo: &str,
    decimal: u8,
    max_units: Option<u64>,
    transferable: bool,
    code: Option<AssetTypeCode>,
) -> Result<AssetTypeCode> {
    let code = code.unwrap_or_else(AssetTypeCode::gen_random);

    let mut rules = AssetRules::default();
    rules.set_decimals(decimal).c(d!())?;
    rules.set_max_units(max_units);
    rules.set_transferable(transferable);

    let mut builder = utils::new_tx_builder().c(d!())?;
    builder
        .add_operation_create_asset(kp, Some(code), rules, memo)
        .c(d!())?;
    utils::gen_fee_op(kp)
        .c(d!())
        .map(|op| builder.add_operation(op))?;

    utils::send_tx(&builder.take_transaction()).map(|_| code)
}

/// Issue a custom asset with specified amount
pub fn issue_asset(
    sk_str: Option<&str>,
    asset: &str,
    amount: u64,
    hidden: bool,
) -> Result<()> {
    let kp = restore_keypair_from_str_with_default(sk_str)?;
    let code = AssetTypeCode::new_from_base64(asset).c(d!())?;
    issue_asset_x(&kp, &code, amount, hidden).c(d!())
}

#[allow(missing_docs)]
pub fn issue_asset_x(
    kp: &XfrKeyPair,
    code: &AssetTypeCode,
    amount: u64,
    hidden: bool,
) -> Result<()> {
    let confidentiality_flags = AssetRecordType::from_flags(hidden, false);

    let mut builder = utils::new_tx_builder().c(d!())?;
    builder
        .add_basic_issue_asset(
            kp,
            code,
            builder.get_seq_id(),
            amount,
            confidentiality_flags,
            &PublicParams::default(),
        )
        .c(d!())?;
    utils::gen_fee_op(kp)
        .c(d!())
        .map(|op| builder.add_operation(op))?;

    utils::send_tx(&builder.take_transaction())
}

/// Show a list of custom asset token created by a findora account
pub fn show_asset(addr: &str) -> Result<()> {
    let pk = wallet::public_key_from_bech32(addr).c(d!())?;
    let assets = utils::get_created_assets(&pk).c(d!())?;
    for asset in assets {
        let base64 = asset.body.asset.code.to_base64();
        let h = hex::encode(asset.body.asset.code.val.0);
        println!("Base64: {}, Hex: {}", base64, h);
    }

    Ok(())
}

/// Builds a transaction for a BAR to ABAR conversion with fees and sends it to network
/// # Arguments
/// * owner_sk - Optional secret key Xfr in json form
/// * target_addr - ABAR receiving AXfr pub key after conversion in base64
/// * owner_enc_key - XPublicKey of receiver in base64 form for OwnerMemo encryption
/// * TxoSID - sid of BAR to convert
pub fn convert_bar2abar(
    owner_sk: Option<&String>,
    target_addr: String,
    owner_enc_key: String,
    txo_sid: &str,
) -> Result<JubjubScalar> {
    // parse sender XfrSecretKey or generate from Mnemonic setup with wallet
    let from = match owner_sk {
        Some(str) => ruc::info!(serde_json::from_str::<XfrSecretKey>(&format!(
            "\"{}\"",
            str
        )))
        .c(d!())?
        .into_keypair(),
        None => get_keypair().c(d!())?,
    };
    // parse receiver AxfrPubKey
    let to = wallet::anon_public_key_from_base64(target_addr.as_str())
        .c(d!("invalid 'target-addr'"))?;
    // parse receiver XPubKey
    let enc_key = wallet::x_public_key_from_base64(owner_enc_key.as_str())
        .c(d!("invalid owner_enc_key"))?;
    let sid = txo_sid.parse::<u64>().c(d!("error parsing TxoSID"))?;

    // Get OpenAssetRecord from given Owner XfrKeyPair and TxoSID
    let oar =
        utils::get_oar(&from, TxoSID(sid)).c(d!("error fetching open asset record"))?;

    // Generate the transaction and transmit it to network
    let r = utils::generate_bar2abar_op(&from, &to, TxoSID(sid), &oar, &enc_key)
        .c(d!("Bar to abar failed"))?;

    Ok(r)
}

/// Convert an ABAR to a Blind Asset Record
/// # Arguments
/// * axfr_secret_key - the anon_secret_key in base64
/// * r               - randomizer of ABAR in base58
/// * dec_key         - XSecretKey for OwnerMemo decryption in base64
/// * to              - Bar receiver's XfrPublicKey pointer
/// * fr              - randomizer of the FRA ABAR to pay fee in base58
/// * confidential_am - if the output BAR should have confidential amount
/// * confidential_ty - if the output BAR should have confidential type
pub fn convert_abar2bar(
    axfr_secret_key: String,
    r: &str,
    dec_key: String,
    to: &XfrPublicKey,
    fr: &str,
    confidential_am: bool,
    confidential_ty: bool,
) -> Result<()> {
    // parse anon keys
    let from = wallet::anon_secret_key_from_base64(axfr_secret_key.as_str())
        .c(d!("invalid 'from-axfr-secret-key'"))?;
    let from_secret_key =
        wallet::x_secret_key_from_base64(dec_key.as_str()).c(d!("invalid dec_key"))?;
    let from_public_key = XPublicKey::from(&from_secret_key);

    // Get the owned ABAR from pub_key and randomizer
    let r = wallet::randomizer_from_base58(r).c(d!())?;
    let randomized_from_pub_key = from.pub_key().randomize(&r);
    let axtxo_abar = utils::get_owned_abars(&randomized_from_pub_key).c(d!())?;

    // get OwnerMemo and Merkle Proof of ABAR
    let owner_memo = utils::get_abar_memo(&axtxo_abar[0].0).c(d!())?.unwrap();
    let mt_leaf_info = utils::get_abar_proof(&axtxo_abar[0].0).c(d!())?.unwrap();
    let mt_leaf_uid = mt_leaf_info.uid;

    // Open ABAR with dec_key and OwnerMemo & attach merkle proof
    let oabar_in = OpenAnonBlindAssetRecordBuilder::from_abar(
        &axtxo_abar[0].1,
        owner_memo,
        &from,
        &from_secret_key,
    )
    .unwrap()
    .mt_leaf_info(mt_leaf_info)
    .build()
    .unwrap();

    // check oabar is unspent. If already spent return error
    // create nullifier
    let n = nullifier(
        &from.randomize(&r),
        oabar_in.get_amount(),
        &oabar_in.get_asset_type(),
        mt_leaf_uid,
    );
    let hash = base64::encode_config(&n.to_bytes(), base64::URL_SAFE);
    // check if hash is present in nullifier set
    let null_status = utils::check_nullifier_hash(&hash)
        .c(d!())?
        .ok_or(d!("The ABAR corresponding to this randomizer is missing"))?;
    if null_status {
        return Err(eg!(
            "The ABAR corresponding to this randomizer is already spent"
        ));
    }

    // Create randomized public key for fee & get ABAR
    let fr = wallet::randomizer_from_base58(fr).c(d!())?;
    let fee_randomized_key = from.pub_key().randomize(&fr);
    let fee_axtxo_abar = utils::get_owned_abars(&fee_randomized_key).c(d!())?;
    // Get Fee OwnerMemo & Merkle Proof
    let fee_owner_memo = utils::get_abar_memo(&fee_axtxo_abar[0].0).c(d!())?.unwrap();
    let fee_mt_leaf_info = utils::get_abar_proof(&fee_axtxo_abar[0].0)
        .c(d!())?
        .unwrap();

    let fee_oabar = OpenAnonBlindAssetRecordBuilder::from_abar(
        &fee_axtxo_abar[0].1,
        fee_owner_memo,
        &from,
        &from_secret_key,
    )
    .unwrap()
    .mt_leaf_info(fee_mt_leaf_info)
    .build()
    .unwrap();

    let mut prng = ChaChaRng::from_entropy();
    let out_fee_oabar = OpenAnonBlindAssetRecordBuilder::new()
        .amount(fee_oabar.get_amount() - ANON_FEE_MIN)
        .asset_type(fee_oabar.get_asset_type())
        .pub_key(from.pub_key())
        .finalize(&mut prng, &from_public_key)
        .unwrap()
        .build()
        .unwrap();

    // Create New AssetRecordType for new BAR
    let art = match (confidential_am, confidential_ty) {
        (true, true) => AssetRecordType::ConfidentialAmount_ConfidentialAssetType,
        (true, false) => AssetRecordType::ConfidentialAmount_NonConfidentialAssetType,
        (false, true) => AssetRecordType::NonConfidentialAmount_ConfidentialAssetType,
        _ => AssetRecordType::NonConfidentialAmount_NonConfidentialAssetType,
    };

    // Build AbarToBar Transaction and submit
    utils::generate_abar2bar_op(&oabar_in, &fee_oabar, &out_fee_oabar, &from, to, art)
        .c(d!())?;

    println!(
        "\x1b[31;01m Fee Remainder Randomizer: {}\x1b[00m",
        wallet::randomizer_to_base58(&out_fee_oabar.get_key_rand_factor())
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("owned_randomizers")
        .expect("cannot open randomizers file");
    std::io::Write::write_all(
        &mut file,
        ("\n".to_owned()
            + &wallet::randomizer_to_base58(&out_fee_oabar.get_key_rand_factor()))
            .as_bytes(),
    )
    .expect("randomizer write failed");
    Ok(())
}

/// Generate OABAR and add anonymous transfer operation
/// # Arguments
/// * axfr_secret_key - AXfrKeyPair in base64 form
/// * r               - Randomizer in base58 form
/// * fee_r           - Randomizer for paying fee
/// * dec_key         - XPublicKey to encrypt OwnerMemo
/// * amount          - amount to transfer
pub fn gen_oabar_add_op(
    axfr_secret_key: String,
    r: &str,
    fee_r: Option<&str>,
    dec_key: String,
    amount: &str,
    to_axfr_public_key: &str,
    to_enc_key: &str,
) -> Result<()> {
    // parse sender keys
    let from = wallet::anon_secret_key_from_base64(axfr_secret_key.as_str())
        .c(d!("invalid 'from-axfr-secret-key'"))?;
    let from_secret_key =
        wallet::x_secret_key_from_base64(dec_key.as_str()).c(d!("invalid dec_key"))?;
    // sender public key to recieve balance after fee
    let from_public_key = XPublicKey::from(&from_secret_key);

    let axfr_amount = amount.parse::<u64>().c(d!("error parsing amount"))?;

    let to = wallet::anon_public_key_from_base64(to_axfr_public_key)
        .c(d!("invalid 'to-axfr-public-key'"))?;
    let enc_key_out =
        wallet::x_public_key_from_base64(to_enc_key).c(d!("invalid to_enc_key"))?;

    let mut randomizers = vec![r];
    if let Some(fra) = fee_r {
        randomizers.push(fra);
    }
    let mut inputs = vec![];
    // For each randomizer add input to transfer operation
    for r in randomizers {
        // generate randomized public key
        let r = wallet::randomizer_from_base58(r).c(d!())?;
        let randomized_from_pub_key = from.pub_key().randomize(&r);

        // get unspent ABARs & their Merkle proof for randomized public key
        let axtxo_abar = utils::get_owned_abars(&randomized_from_pub_key).c(d!())?;
        let owner_memo = utils::get_abar_memo(&axtxo_abar[0].0).c(d!())?.unwrap();
        let mt_leaf_info = utils::get_abar_proof(&axtxo_abar[0].0).c(d!())?.unwrap();
        let mt_leaf_uid = mt_leaf_info.uid;

        // Create Open ABAR from input information
        let oabar_in = OpenAnonBlindAssetRecordBuilder::from_abar(
            &axtxo_abar[0].1,
            owner_memo,
            &from,
            &from_secret_key,
        )
        .unwrap()
        .mt_leaf_info(mt_leaf_info)
        .build()
        .unwrap();

        // check oabar is unspent.
        let n = nullifier(
            &from.randomize(&r),
            oabar_in.get_amount(),
            &oabar_in.get_asset_type(),
            mt_leaf_uid,
        );
        let hash = base64::encode_config(&n.to_bytes(), base64::URL_SAFE);
        let null_status = utils::check_nullifier_hash(&hash).c(d!())?.ok_or(d!(
            "The ABAR corresponding to this randomizer is missing {:?}",
            wallet::randomizer_to_base58(&r)
        ))?;
        if null_status {
            return Err(eg!(
                "The ABAR corresponding to this randomizer is already spent {:?}",
                wallet::randomizer_to_base58(&r)
            ));
        }
        inputs.push(oabar_in);
    }

    let froms = vec![from; inputs.len()];

    // build output
    let mut prng = ChaChaRng::from_entropy();
    let oabar_out = OpenAnonBlindAssetRecordBuilder::new()
        .amount(axfr_amount)
        .asset_type(inputs[0].get_asset_type())
        .pub_key(to)
        .finalize(&mut prng, &enc_key_out)
        .unwrap()
        .build()
        .unwrap();

    let r_out = oabar_out.get_key_rand_factor();
    let mut builder: TransactionBuilder = new_tx_builder().c(d!())?;
    let (_, note, rem_oabars) = builder
        .add_operation_anon_transfer_fees_remainder(
            &inputs,
            &[oabar_out],
            &froms,
            from_public_key,
        )
        .c(d!())?;

    send_tx(&builder.take_transaction()).c(d!())?;

    println!(
        "\x1b[31;01m Randomizer: {}\x1b[00m",
        wallet::randomizer_to_base58(&r_out)
    );
    // Append receiver's randomizer to `sent_randomizers` file
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("sent_randomizers")
        .expect("cannot open randomizers file");
    std::io::Write::write_all(
        &mut file,
        ("\n".to_owned() + &wallet::randomizer_to_base58(&r_out)).as_bytes(),
    )
    .expect("randomizer write failed");

    for rem_oabar in rem_oabars.iter() {
        println!(
            "\x1b[31;01m Remainder Randomizer: {}\x1b[00m",
            wallet::randomizer_to_base58(&rem_oabar.get_key_rand_factor())
        );
    }

    // Append sender's fee balance randomizer to `owned_randomizers` file
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("owned_randomizers")
        .expect("cannot open randomizers file");
    for rem_oabar in rem_oabars.iter() {
        std::io::Write::write_all(
            &mut file,
            ("\n".to_owned()
                + &wallet::randomizer_to_base58(&rem_oabar.get_key_rand_factor()))
                .as_bytes(),
        )
        .expect("randomizer write failed");
    }

    println!("Signed AxfrNote: {:?}", serde_json::to_string_pretty(&note));
    Ok(())
}

/// Batch anon transfer - Generate OABAR and add anonymous transfer operation
/// Note - if multiple anon keys are used, we consider the last key in the list for remainder.
/// # Arguments
/// * axfr_secret_keys    - list of secret keys for senders' ABAR UTXOs
/// * dec_keys            - list of decryption keys for senders' ABARs
/// * to_axfr_public_keys - receiver AXfr Public keys
/// * to_enc_keys         - List of receiver Encryption keys
/// * randomizers         - List of sender randomizers in base58 format
/// * amounts             - List of receiver amounts
/// * assets              - List of receiver Asset Types
/// returns an error if Operation build fails
pub fn gen_oabar_add_op_x(
    axfr_secret_keys: Vec<AXfrKeyPair>,
    dec_keys: Vec<XSecretKey>,
    to_axfr_public_keys: Vec<AXfrPubKey>,
    to_enc_keys: Vec<XPublicKey>,
    randomizers: Vec<String>,
    amounts: Vec<String>,
    assets: Vec<AssetTypeCode>,
) -> Result<()> {
    let sender_count = axfr_secret_keys.len();
    let receiver_count = to_axfr_public_keys.len();

    // check if input counts tally
    if sender_count != randomizers.len()
        || sender_count != dec_keys.len()
        || receiver_count != amounts.len()
        || receiver_count != assets.len()
    {
        return Err(eg!(
            "The Parameters: from-sk/dec-keys/randomizers or to-pk/to-enc-keys not match!"
        ));
    }

    // Create Input Open Abars with input keys, radomizers and Owner memos
    let mut oabars_in = Vec::new();
    for i in 0..sender_count {
        // Create randomized public key
        let from = &axfr_secret_keys[i];
        let from_secret_key = &dec_keys[i];
        let r = wallet::randomizer_from_base58(randomizers[i].as_str()).c(d!())?;
        let from_pub_key_randomized = from.pub_key().randomize(&r);

        // Get OwnerMemo
        let axtxo_abar = utils::get_owned_abars(&from_pub_key_randomized).c(d!())?;
        let owner_memo = utils::get_abar_memo(&axtxo_abar[0].0).c(d!())?.unwrap();
        // Get Merkle Proof
        let mt_leaf_info = utils::get_abar_proof(&axtxo_abar[0].0).c(d!())?.unwrap();
        let mt_leaf_uid = mt_leaf_info.uid;

        // Build Abar
        let oabar_in = OpenAnonBlindAssetRecordBuilder::from_abar(
            &axtxo_abar[0].1,
            owner_memo,
            from,
            from_secret_key,
        )
        .unwrap()
        .mt_leaf_info(mt_leaf_info)
        .build()
        .unwrap();

        // check oabar is unspent.
        let n = nullifier(
            &from.randomize(&r),
            oabar_in.get_amount(),
            &oabar_in.get_asset_type(),
            mt_leaf_uid,
        );
        let hash = base64::encode_config(&n.to_bytes(), base64::URL_SAFE);
        let null_status = utils::check_nullifier_hash(&hash)
            .c(d!())?
            .ok_or(d!("The ABAR corresponding to this randomizer is missing"))?;
        if null_status {
            return Err(eg!(
                "The ABAR corresponding to this randomizer is already spent"
            ));
        }

        oabars_in.push(oabar_in);
    }

    // Create output Open ABARs
    let mut oabars_out = Vec::new();
    for i in 0..receiver_count {
        let mut prng = ChaChaRng::from_entropy();
        let to = to_axfr_public_keys[i];
        let enc_key_out = &to_enc_keys[i];
        let axfr_amount = amounts[i].parse::<u64>().c(d!("error parsing amount"))?;
        let asset_type = assets[i];

        let oabar_out = OpenAnonBlindAssetRecordBuilder::new()
            .amount(axfr_amount)
            .asset_type(asset_type.val)
            .pub_key(to)
            .finalize(&mut prng, enc_key_out)
            .unwrap()
            .build()
            .unwrap();

        oabars_out.push(oabar_out);
    }

    // Add a output for fees balance
    let from_encryption_key = XPublicKey::from(dec_keys.last().unwrap());
    let mut builder: TransactionBuilder = new_tx_builder().c(d!())?;
    let (_, note, rem_oabars) = builder
        .add_operation_anon_transfer_fees_remainder(
            &oabars_in[..],
            &oabars_out[..],
            &axfr_secret_keys,
            from_encryption_key,
        )
        .c(d!())?;

    // Send the transaction to the network
    send_tx(&builder.take_transaction()).c(d!())?;

    // Append receiver's randomizer to `sent_randomizers` file
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("sent_randomizers")
        .expect("cannot open sent_randomizers file");
    for oabar_out in oabars_out {
        let r_out = oabar_out.get_key_rand_factor();
        println!(
            "\x1b[31;01m Randomizer: {}\x1b[00m",
            wallet::randomizer_to_base58(&r_out)
        );
        std::io::Write::write_all(
            &mut file,
            ("\n".to_owned() + &wallet::randomizer_to_base58(&r_out)).as_bytes(),
        )
        .expect("randomizer write failed");
    }

    // Append sender's fee balance randomizer to `owned_randomizers` file
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open("owned_randomizers")
        .expect("cannot open owned_randomizers file");
    for rem_oabar in rem_oabars.iter() {
        println!(
            "\x1b[31;01m Remainder Randomizer: {}\x1b[00m",
            wallet::randomizer_to_base58(&rem_oabar.get_key_rand_factor())
        );
        std::io::Write::write_all(
            &mut file,
            ("\n".to_owned()
                + &wallet::randomizer_to_base58(&rem_oabar.get_key_rand_factor()))
                .as_bytes(),
        )
        .expect("randomizer write failed");
    }

    println!("Signed AxfrNote: {:?}", serde_json::to_string_pretty(&note));
    Ok(())
}

/// Get merkle proof - Generate MTLeafInfo from ATxoSID
pub fn get_mtleaf_info(atxo_sid: &str) -> Result<MTLeafInfo> {
    let asid = atxo_sid.parse::<u64>().c(d!("error parsing ATxoSID"))?;
    let mt_leaf_info = utils::get_abar_proof(&ATxoSID(asid))
        .c(d!("error fetching abar proof"))?
        .unwrap();
    Ok(mt_leaf_info)
}

/// Fetch Owned ABARs from query server
pub fn get_owned_abars(p: &AXfrPubKey) -> Result<Vec<(ATxoSID, AnonBlindAssetRecord)>> {
    utils::get_owned_abars(p)
}

/// Get the Abar Memo by ATxoSID
pub fn get_abar_memo(uid: &ATxoSID) -> Result<Option<OwnerMemo>> {
    utils::get_abar_memo(uid).c(d!())
}

/// Fetches list of owned TxoSIDs from LedgerStatus
pub fn get_owned_utxos() -> Result<Vec<(TxoSID, XfrAmount, XfrAssetType)>> {
    let kp = get_keypair().c(d!())?;

    let list = utils::get_owned_utxos(&kp.pub_key)?
        .iter()
        .map(|a| {
            let record = a.1.clone().0 .0.record;
            (*a.0, record.amount, record.asset_type)
        })
        .collect();

    Ok(list)
}

/// Check the spending status of an ABAR from AnonKeys and randomizer
pub fn check_abar_status(
    from: AXfrKeyPair,
    r: JubjubScalar,
    from_secret_key: XSecretKey,
    axtxo_abar: Vec<(ATxoSID, AnonBlindAssetRecord)>,
) -> Result<()> {
    let diversified_from_key = from.randomize(&r);
    let owner_memo = utils::get_abar_memo(&axtxo_abar[0].0).c(d!())?.unwrap();
    let mt_leaf_info = utils::get_abar_proof(&axtxo_abar[0].0).c(d!())?.unwrap();
    let mt_leaf_uid = mt_leaf_info.uid;

    let oabar = OpenAnonBlindAssetRecordBuilder::from_abar(
        &axtxo_abar[0].1,
        owner_memo,
        &from,
        &from_secret_key,
    )
    .unwrap()
    .mt_leaf_info(mt_leaf_info)
    .build()
    .unwrap();

    let n = nullifier(
        &diversified_from_key,
        oabar.get_amount(),
        &oabar.get_asset_type(),
        mt_leaf_uid,
    );
    let hash = base64::encode_config(&n.to_bytes(), base64::URL_SAFE);
    let null_status = utils::check_nullifier_hash(&hash).c(d!())?.unwrap();
    if null_status {
        println!("The ABAR corresponding to this randomizer is already spent");
    } else {
        println!("The ABAR corresponding to this randomizer is unspent and has a balance {:?}", oabar.get_amount());
    }
    Ok(())
}

/// Prints a dainty list of Abar info with spent status for a given AxfrKeyPair and a list of
/// randomizers.
pub fn anon_balance(
    axfr_secret_key: AXfrKeyPair,
    axfr_public_key: AXfrPubKey,
    dec_key: XSecretKey,
    randomizers_list: &str,
) -> Result<()> {
    let axfr_public_key_str = wallet::anon_public_key_to_base64(&axfr_public_key);
    println!(
        "Abar data for pubkey: {}, randomizers: {}",
        axfr_public_key_str, randomizers_list
    );
    println!();
    println!(
        "{0: <8} | {1: <18} | {2: <45} | {3: <9} | {4: <45} | {5: <45}",
        "ATxoSID", "Amount", "AssetType", "IsSpent", "AXfrPublicKey", "Randomizer"
    );
    println!("{:-^1$}", "", 184);
    randomizers_list
        .split(',')
        .try_for_each(|r| -> ruc::Result<()> {
            let randomizer = wallet::randomizer_from_base58(r).c(d!())?;
            let derived_public_key = axfr_public_key.randomize(&randomizer);
            let list = get_owned_abars(&derived_public_key).c(d!())?;
            list.iter().try_for_each(|(sid, abar)| -> ruc::Result<()> {
                let memo = get_abar_memo(sid).unwrap().unwrap();
                let oabar = OpenAnonBlindAssetRecordBuilder::from_abar(
                    abar,
                    memo,
                    &axfr_secret_key,
                    &dec_key,
                )
                .unwrap()
                .build()
                .unwrap();

                let n = nullifier(
                    &axfr_secret_key.randomize(&randomizer),
                    oabar.get_amount(),
                    &oabar.get_asset_type(),
                    sid.0,
                );
                let hash = base64::encode_config(&n.to_bytes(), base64::URL_SAFE);
                let null_status = utils::check_nullifier_hash(&hash).c(d!())?.unwrap();
                println!(
                    "{0: <8} | {1: <18} | {2: <45} | {3: <9} | {4: <45} | {5: <45}",
                    sid.0,
                    oabar.get_amount(),
                    AssetTypeCode {
                        val: oabar.get_asset_type()
                    }
                    .to_base64(),
                    null_status,
                    axfr_public_key_str,
                    r
                );
                Ok(())
            })?;
            Ok(())
        })?;

    Ok(())
}

/// Return the built version.
pub fn version() -> &'static str {
    concat!(env!("VERGEN_SHA"), " ", env!("VERGEN_BUILD_DATE"))
}

///operation to replace the staker.
pub fn replace_staker(
    target_pubkey: XfrPublicKey,
    new_td_addr_pk: Option<(Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    let keypair = get_keypair()?;

    let mut builder = utils::new_tx_builder().c(d!())?;

    utils::gen_fee_op(&keypair).c(d!()).map(|op| {
        builder.add_operation(op);
    })?;

    builder.add_operation_replace_staker(&keypair, target_pubkey, new_td_addr_pk)?;
    let tx = builder.take_transaction();
    utils::send_tx(&tx).c(d!())?;
    Ok(())
}
