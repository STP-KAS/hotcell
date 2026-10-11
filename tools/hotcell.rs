//! hotcell: one contended KCC-20 public-mint cell for testnet-10 drills.
//!
//! Builds the genesis (PublicMint + TokenSeed) and a short chain of mints that
//! each issue 1 unit. Optionally builds a higher-fee replacement of the last
//! mint (same inputs) so a client can rehearse replace-by-fee on a covenant
//! spend. The payer key is read from HC_KEY. This module never prints a key.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::str::FromStr;

use argent::artifact::Artifact;
use argent_runtime::{args, state, CovenantOutput, EntryCall, InputSigScript, TxBuilder, TxContext};
use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::mass::MassCalculator;
use kaspa_consensus_core::tx::{
    CovenantBinding, MutableTransaction, ScriptPublicKey, Transaction, TransactionId, TransactionOutpoint,
    UtxoEntry,
};
use secp256k1::{Keypair, Secp256k1, SecretKey};

use super::support::{sign_input, DemoResult};
use super::TokenState;

pub const NAME: &[u8] = b"hotcell";
pub const LAUNCH_PATH: &str = "launch::hotcell-v1";
pub const SUPPLY: i64 = 1_000_000;
pub const LIMIT: i64 = 1;
const KAS: u64 = 100_000_000;
const STORAGE_LIMIT: u64 = 500_000;
const BLOCK_COMPUTE: u64 = 500_000;
const BLOCK_TRANSIENT: u64 = 1_000_000;

struct Coin {
    outpoint: TransactionOutpoint,
    entry: UtxoEntry,
}

struct Ready {
    tx: Transaction,
    storage: u64,
    compute: u64,
    transient: u64,
    fee: u64,
}

pub fn run(artifact: &Artifact) -> DemoResult<()> {
    let extension = *blake3::hash(NAME).as_bytes();
    let (payer, payer_public) = load_key()?;
    let rate = env_u64("HC_FEE_RATE", 1, 10_000)?;
    let mints = env_u64("HC_MINTS", 1, 50)? as usize;
    let bump = match std::env::var("HC_BUMP_RATE") {
        Ok(text) if !text.trim().is_empty() => Some(text.trim().parse::<u64>()?),
        _ => None,
    };
    if let Some(b) = bump {
        if b <= rate || b > 10_000 {
            return Err("HC_BUMP_RATE must be above HC_FEE_RATE and at most 10000".into());
        }
    }
    let out_dir = std::env::var("HC_OUT").map_err(|_| "HC_OUT")?;
    fs::create_dir_all(&out_dir)?;
    let tx_path = format!("{out_dir}/txs.jsonl");
    fs::write(&tx_path, "")?;
    let coin = load_funding(&payer_public)?;
    let builder = TxBuilder::new(artifact)?;

    if let Ok(path) = std::env::var("HC_CONTINUE") {
        return continue_mints(&builder, &payer, &payer_public, &extension, &coin, rate, mints, bump, &path, &tx_path, &out_dir);
    }
    let minter_owner = payer_public;
    // Genesis: one PublicMint (supply, limit 1) and one TokenSeed, same group.
    let genesis = fit_fee(rate, |fee| {
        let change = coin.entry.amount.checked_sub(2 * KAS).and_then(|v| v.checked_sub(fee)).ok_or("change")?;
        let tx = builder.build(
            &TxContext::new()
                .input(
                    coin.outpoint,
                    coin.entry.clone(),
                    InputSigScript::with_transaction(|tx, index| p2pk_signature(&sign_input(tx, index, &payer))),
                    0,
                )
                .actor_genesis_output(0, LAUNCH_PATH, "PublicMint", minter_state(SUPPLY, &payer_public, &extension), KAS)
                .actor_genesis_output(0, LAUNCH_PATH, "TokenSeed", seed_state(&payer_public, &extension), KAS)
                .output(p2pk(&payer_public), None, change),
        )?;
        commit(tx, &[coin.entry.clone()])
    })?;
    let mut minter = CovenantOutput::from_tx(&genesis.tx, 0)?;
    let covenant_id = minter.covenant_id;
    append_tx(&tx_path, "genesis", "0", &genesis)?;
    let mut pay = change_coin(&genesis.tx, 2)?;
    let mut remaining = SUPPLY;
    let mut summary = format!(
        "{{\"name\":\"hotcell\",\"launchPath\":\"{LAUNCH_PATH}\",\"extension\":\"{}\",\"covenantId\":\"{}\",\"supply\":{SUPPLY},\"perMint\":{LIMIT},\"feeRate\":{rate},\"genesisFee\":{},\"genesisStorage\":{},\"mints\":[",
        hex(&extension),
        covenant_id,
        genesis.fee,
        genesis.storage
    );
    let mut minter_src: Transaction = genesis.tx.clone();
    let mut last_inputs: Option<(Transaction, Coin, i64)> = None;
    for n in 0..mints {
        let ready = build_mint(&builder, &payer, &payer_public, &minter_owner, &extension, &minter, &pay, remaining, rate)?;
        append_tx(&tx_path, "mint", &(n + 1).to_string(), &ready)?;
        if n > 0 {
            summary.push(',');
        }
        summary.push_str(&format!(
            "{{\"n\":{},\"fee\":{},\"storage\":{},\"compute\":{},\"remainingAfter\":{}}}",
            n + 1,
            ready.fee,
            ready.storage,
            ready.compute,
            remaining - LIMIT
        ));
        last_inputs = Some((minter_src.clone(), Coin { outpoint: pay.outpoint, entry: pay.entry.clone() }, remaining));
        minter = CovenantOutput::from_tx(&ready.tx, 0)?;
        minter_src = ready.tx.clone();
        pay = change_coin(&ready.tx, 2)?;
        remaining -= LIMIT;
    }
    summary.push(']');
    if let (Some(b), Some((src, c, r))) = (bump, last_inputs) {
        let m = CovenantOutput::from_tx(&src, 0)?;
        let ready = build_mint(&builder, &payer, &payer_public, &minter_owner, &extension, &m, &c, r, b)?;
        append_tx(&tx_path, "bump", &mints.to_string(), &ready)?;
        summary.push_str(&format!(",\"bump\":{{\"replaces\":{mints},\"feeRate\":{b},\"fee\":{}}}", ready.fee));
    }
    summary.push_str("}\n");
    fs::write(format!("{out_dir}/summary.json"), summary)?;
    println!("covenant {covenant_id}");
    println!("built genesis + {mints} mints at feeRate {rate}");
    Ok(())
}

/// Continue mode: mint into an existing hotcell family from its current minter output.
/// HC_CONTINUE names a file with covenant=, txid=, index=, value=, script=, remaining=, owner= lines
/// (owner is the minter's deposit owner, i.e. the launcher's x-only public key; the minted unit goes to HC_KEY).
#[allow(clippy::too_many_arguments)]
fn continue_mints(
    builder: &TxBuilder<'_>,
    payer: &Keypair,
    payer_public: &[u8; 32],
    extension: &[u8; 32],
    coin: &Coin,
    rate: u64,
    mints: usize,
    bump: Option<u64>,
    path: &str,
    tx_path: &str,
    out_dir: &str,
) -> DemoResult<()> {
    let text = fs::read_to_string(path)?;
    let covenant_id = kaspa_consensus_core::Hash::from_str(value_of(&text, "covenant")?)?;
    let owner_bytes = hex_decode(value_of(&text, "owner")?)?;
    let minter_owner: [u8; 32] = owner_bytes.as_slice().try_into().map_err(|_| "owner must be 32 bytes")?;
    let mut remaining: i64 = value_of(&text, "remaining")?.parse()?;
    let mut minter = CovenantOutput {
        index: value_of(&text, "index")?.parse()?,
        covenant_id,
        outpoint: TransactionOutpoint::new(TransactionId::from_str(value_of(&text, "txid")?)?, value_of(&text, "index")?.parse()?),
        utxo: UtxoEntry::new(
            value_of(&text, "value")?.parse()?,
            ScriptPublicKey::from_vec(0, hex_decode(value_of(&text, "script")?)?),
            0,
            false,
            Some(covenant_id),
        ),
    };
    let mut pay = Coin { outpoint: coin.outpoint, entry: coin.entry.clone() };
    let mut last: Option<(CovenantOutput, Coin, i64)> = None;
    let mut summary = format!("{{\"name\":\"hotcell\",\"mode\":\"continue\",\"covenantId\":\"{covenant_id}\",\"feeRate\":{rate},\"mints\":[");
    for n in 0..mints {
        if remaining < LIMIT {
            return Err("minter exhausted".into());
        }
        let ready = build_mint(builder, payer, payer_public, &minter_owner, extension, &minter, &pay, remaining, rate)?;
        append_tx(tx_path, "mint", &(n + 1).to_string(), &ready)?;
        if n > 0 {
            summary.push(',');
        }
        summary.push_str(&format!("{{\"n\":{},\"fee\":{},\"storage\":{},\"remainingAfter\":{}}}", n + 1, ready.fee, ready.storage, remaining - LIMIT));
        last = Some((minter.clone_output(), Coin { outpoint: pay.outpoint, entry: pay.entry.clone() }, remaining));
        minter = CovenantOutput::from_tx(&ready.tx, 0)?;
        pay = change_coin(&ready.tx, 2)?;
        remaining -= LIMIT;
    }
    summary.push(']');
    if let (Some(b), Some((m, c, r))) = (bump, last) {
        let ready = build_mint(builder, payer, payer_public, &minter_owner, extension, &m, &c, r, b)?;
        append_tx(tx_path, "bump", &mints.to_string(), &ready)?;
        summary.push_str(&format!(",\"bump\":{{\"replaces\":{mints},\"feeRate\":{b},\"fee\":{}}}", ready.fee));
    }
    summary.push_str("}\n");
    fs::write(format!("{out_dir}/summary.json"), summary)?;
    println!("covenant {covenant_id}");
    println!("built {mints} continue-mode mints at feeRate {rate}; next minter is output 0 of the last mint");
    Ok(())
}

trait CloneOutput {
    fn clone_output(&self) -> CovenantOutput;
}

impl CloneOutput for CovenantOutput {
    fn clone_output(&self) -> CovenantOutput {
        CovenantOutput { index: self.index, covenant_id: self.covenant_id, outpoint: self.outpoint, utxo: self.utxo.clone() }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_mint(
    builder: &TxBuilder<'_>,
    payer: &Keypair,
    payer_public: &[u8; 32],
    minter_owner: &[u8; 32],
    extension: &[u8; 32],
    minter: &CovenantOutput,
    coin: &Coin,
    remaining: i64,
    rate: u64,
) -> DemoResult<Ready> {
    fit_fee(rate, |fee| {
        let change_value = coin.entry.amount.checked_sub(KAS).and_then(|v| v.checked_sub(fee)).ok_or("change")?;
        let binding = CovenantBinding::new(0, minter.covenant_id);
        let minted = token_state(LIMIT, payer_public, extension);
        let tx = builder.build(
            &TxContext::new()
                .actor_input(
                    "PublicMint",
                    minter_state(remaining, minter_owner, extension),
                    EntryCall::new("mint").args(args!(minted.clone())),
                    minter.outpoint,
                    minter.utxo.clone(),
                    0,
                )
                .input(
                    coin.outpoint,
                    coin.entry.clone(),
                    InputSigScript::with_transaction(|tx, index| p2pk_signature(&sign_input(tx, index, payer))),
                    0,
                )
                .actor_output("PublicMint", minter_state(remaining - LIMIT, minter_owner, extension), binding, minter.utxo.amount)
                .actor_output("KCC20", minted, binding, KAS)
                .output(p2pk(payer_public), None, change_value),
        )?;
        commit(tx, &[minter.utxo.clone(), coin.entry.clone()])
    })
}

fn env_u64(key: &str, min: u64, max: u64) -> DemoResult<u64> {
    let v: u64 = std::env::var(key).map_err(|_| key.to_string())?.trim().parse()?;
    if v < min || v > max {
        return Err(format!("{key} out of range").into());
    }
    Ok(v)
}

fn load_key() -> DemoResult<(Keypair, [u8; 32])> {
    let path = std::env::var("HC_KEY").map_err(|_| "HC_KEY")?;
    let secret = hex_decode(fs::read_to_string(&path)?.trim())?;
    if secret.len() != 32 {
        return Err("key length".into());
    }
    let secret_key = SecretKey::from_slice(&secret)?;
    let key = Keypair::from_secret_key(&Secp256k1::new(), &secret_key);
    let public = key.x_only_public_key().0.serialize();
    let expect = std::env::var("HC_PAYER_ADDRESS").map_err(|_| "HC_PAYER_ADDRESS")?;
    let address = Address::try_from(expect.trim())?;
    if address.prefix != Prefix::Testnet || address.version != Version::PubKey || address.payload.as_slice() != public {
        return Err("payer key does not match HC_PAYER_ADDRESS (testnet p2pk only)".into());
    }
    Ok((key, public))
}

fn load_funding(payer_public: &[u8; 32]) -> DemoResult<Coin> {
    let path = std::env::var("HC_FUNDING").map_err(|_| "HC_FUNDING")?;
    let text = fs::read_to_string(path)?;
    let txid = value_of(&text, "txid")?;
    let index: u32 = value_of(&text, "index")?.parse()?;
    let amount: u64 = value_of(&text, "amount")?.parse()?;
    let script = hex_decode(value_of(&text, "script")?)?;
    let coinbase = value_of(&text, "coinbase")? == "1";
    let block_daa: u64 = value_of(&text, "blockDaa")?.parse()?;
    if script.as_slice() != p2pk(payer_public).script() {
        return Err("funding script".into());
    }
    if amount < 2 * KAS {
        return Err("funding below 2 KAS".into());
    }
    Ok(Coin {
        outpoint: TransactionOutpoint::new(TransactionId::from_str(txid)?, index),
        entry: UtxoEntry::new(amount, ScriptPublicKey::from_vec(0, script), block_daa, coinbase, None),
    })
}

fn minter_state(remaining: i64, owner: &[u8; 32], extension: &[u8; 32]) -> TokenState {
    state! {
        remaining: remaining,
        mint_amount: LIMIT,
        owner: owner.to_vec(),
        extension_commitment: extension.to_vec()
    }
}

fn seed_state(owner: &[u8; 32], extension: &[u8; 32]) -> TokenState {
    state! {
        owner: owner.to_vec(),
        extension_commitment: extension.to_vec()
    }
}

fn token_state(amount: i64, owner: &[u8; 32], extension: &[u8; 32]) -> TokenState {
    state! {
        amount: amount,
        owner: owner.to_vec(),
        owner_scheme: 0u8,
        borrow_scheme: 0u8,
        borrow_guard: vec![0u8; 32],
        extension_commitment: extension.to_vec()
    }
}

fn append_tx(path: &str, kind: &str, detail: &str, ready: &Ready) -> DemoResult<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(
        file,
        "{{\"kind\":\"{kind}\",\"detail\":\"{detail}\",\"storage\":{},\"compute\":{},\"transient\":{},\"fee\":\"{}\",\"tx\":{}}}",
        ready.storage,
        ready.compute,
        ready.transient,
        ready.fee,
        tx_json(&ready.tx)
    )?;
    Ok(())
}

fn value_of<'a>(text: &'a str, key: &str) -> DemoResult<&'a str> {
    for line in text.lines() {
        if let Some(value) = line.strip_prefix(&format!("{key}=")) {
            return Ok(value.trim());
        }
    }
    Err(format!("funding {key}").into())
}

fn p2pk(public_key: &[u8; 32]) -> ScriptPublicKey {
    let mut script = Vec::with_capacity(34);
    script.push(32);
    script.extend_from_slice(public_key);
    script.push(0xac);
    ScriptPublicKey::from_vec(0, script)
}

fn p2pk_signature(raw: &[u8]) -> Vec<u8> {
    let mut script = Vec::with_capacity(1 + raw.len());
    script.push(u8::try_from(raw.len()).expect("signature length"));
    script.extend_from_slice(raw);
    script
}


fn commit(mut tx: Transaction, entries: &[UtxoEntry]) -> DemoResult<Ready> {
    let calculator = MassCalculator::new(1, 10, 1_000_000_000_000);
    let populated = MutableTransaction::with_entries(tx.clone(), entries.to_vec());
    let storage = calculator.calc_contextual_masses(&populated.as_verifiable()).ok_or("storage mass")?.storage_mass;
    if storage > STORAGE_LIMIT {
        return Err(format!("storage {storage}").into());
    }
    tx.set_storage_mass(storage);
    tx.finalize();
    let masses = calculator.calc_non_contextual_masses(&tx);
    if masses.compute_mass > BLOCK_COMPUTE || masses.transient_mass > BLOCK_TRANSIENT {
        return Err("mass".into());
    }
    let input_value: u64 = entries.iter().map(|entry| entry.amount).sum();
    let output_value: u64 = tx.outputs.iter().map(|output| output.value).sum();
    if input_value < output_value {
        return Err("values".into());
    }
    Ok(Ready {
        tx,
        storage,
        compute: masses.compute_mass,
        transient: masses.transient_mass,
        fee: input_value - output_value,
    })
}

fn required_fee(item: &Ready, rate: u64) -> u64 {
    let relay = item.compute.max(item.transient.div_ceil(2));
    item.storage.max(relay).saturating_mul(rate).max(1)
}

fn fit_fee(rate: u64, build: impl Fn(u64) -> DemoResult<Ready>) -> DemoResult<Ready> {
    let mut fee = rate.saturating_mul(20_000).max(1);
    let mut last = String::from("no fee");
    for _ in 0..8 {
        match build(fee) {
            Ok(ready) => {
                let need = required_fee(&ready, rate);
                if ready.fee >= need {
                    return Ok(ready);
                }
                last = format!("need {need} had {}", ready.fee);
                fee = need;
            }
            Err(error) => {
                last = error.to_string();
                if last.contains("storage") {
                    return Err(last.into());
                }
                fee = fee.saturating_mul(2).max(fee.saturating_add(1_000_000));
            }
        }
    }
    Err(last.into())
}

fn change_coin(tx: &Transaction, index: u32) -> DemoResult<Coin> {
    let output = tx.outputs.get(index as usize).ok_or("change index")?;
    if output.covenant.is_some() {
        return Err("change is a covenant".into());
    }
    Ok(Coin {
        outpoint: TransactionOutpoint::new(tx.id(), index),
        entry: UtxoEntry::new(output.value, output.script_public_key.clone(), 0, false, None),
    })
}

fn tx_json(tx: &Transaction) -> String {
    let raw = borsh::to_vec(tx).expect("borsh");
    let mut inputs = String::from("[");
    for (index, input) in tx.inputs.iter().enumerate() {
        if index > 0 {
            inputs.push(',');
        }
        inputs.push_str(&format!(
            "{{\"transactionId\":\"{}\",\"index\":{},\"sequence\":\"{}\",\"sigOpCount\":{},\"computeBudget\":{},\"signatureScript\":\"{}\"}}",
            input.previous_outpoint.transaction_id,
            input.previous_outpoint.index,
            input.sequence,
            input.compute_commit.sig_op_count().unwrap_or(0),
            input.compute_commit.compute_budget().unwrap_or(0),
            hex(&input.signature_script)
        ));
    }
    inputs.push(']');
    let mut outputs = String::from("[");
    for (index, output) in tx.outputs.iter().enumerate() {
        if index > 0 {
            outputs.push(',');
        }
        let covenant = match output.covenant {
            Some(binding) => format!(
                "{{\"authorizingInput\":{},\"covenantId\":\"{}\"}}",
                binding.authorizing_input, binding.covenant_id
            ),
            None => "null".to_string(),
        };
        outputs.push_str(&format!(
            "{{\"value\":\"{}\",\"scriptVersion\":{},\"script\":\"{}\",\"covenant\":{covenant}}}",
            output.value,
            output.script_public_key.version(),
            hex(output.script_public_key.script())
        ));
    }
    outputs.push(']');
    format!(
        "{{\"id\":\"{}\",\"version\":{},\"lockTime\":\"{}\",\"gas\":\"{}\",\"subnetworkId\":\"{}\",\"payload\":\"{}\",\"storageMass\":\"{}\",\"inputs\":{inputs},\"outputs\":{outputs},\"borsh\":\"{}\"}}",
        tx.id(),
        tx.version,
        tx.lock_time,
        tx.gas,
        hex(tx.subnetwork_id.as_bytes()),
        hex(&tx.payload),
        tx.storage_mass(),
        hex(&raw)
    )
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> DemoResult<Vec<u8>> {
    if text.len() % 2 != 0 {
        return Err("hex".into());
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let high = hex_value(bytes[index])?;
        let low = hex_value(bytes[index + 1])?;
        out.push((high << 4) | low);
        index += 2;
    }
    Ok(out)
}

fn hex_value(byte: u8) -> DemoResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("hex".into()),
    }
}
