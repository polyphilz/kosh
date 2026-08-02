use std::{env, error::Error, fs, process};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use minisign_verify::{PublicKey, Signature};

fn main() {
    if let Err(error) = verify_updater_signature() {
        eprintln!("updater signature verification failed: {error}");
        process::exit(1);
    }
}

fn verify_updater_signature() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let archive_path = take_argument(&mut arguments, "updater archive path")?;
    let signature_path = take_argument(&mut arguments, "updater signature path")?;
    let config_path = take_argument(&mut arguments, "Tauri configuration path")?;
    if arguments.next().is_some() {
        return Err("unexpected extra updater signature verifier argument".into());
    }

    let configuration: serde_json::Value = serde_json::from_str(&fs::read_to_string(config_path)?)?;
    let encoded_public_key = configuration
        .pointer("/plugins/updater/pubkey")
        .and_then(serde_json::Value::as_str)
        .ok_or("Tauri updater public key is missing")?;
    let public_key_text = String::from_utf8(STANDARD.decode(encoded_public_key)?)?;
    let encoded_signature = fs::read_to_string(&signature_path)?;
    let signature_text = String::from_utf8(STANDARD.decode(encoded_signature.trim())?)?;
    let public_key = PublicKey::decode(&public_key_text)?;
    let signature = Signature::decode(&signature_text)?;
    let archive = fs::read(&archive_path)?;

    public_key.verify(&archive, &signature, true)?;
    println!("Updater signature passed: {archive_path}");
    Ok(())
}

fn take_argument(
    arguments: &mut impl Iterator<Item = String>,
    label: &str,
) -> Result<String, Box<dyn Error>> {
    arguments
        .next()
        .ok_or_else(|| format!("missing {label}").into())
}
