//! Signs deskMonk's update manifest (see src/update.rs); run on the release machine.
//!   cargo run --example sign_update -- keygen KEYFILE               new release key; prints the public key for update.rs
//!   cargo run --example sign_update -- sign KEYFILE docs/update.json  writes docs/update.json.sig
//! KEYFILE holds the private key (hex). Keep it private and backed up: without it no installed copy accepts an update.

use ed25519_dalek::{Signer, SigningKey};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key_of = |path: &str| {
        let seed = deskmonk::update::unhex(std::fs::read_to_string(path).expect("read key").trim()).expect("hex key");
        SigningKey::from_bytes(&seed.try_into().expect("32-byte key"))
    };
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["keygen", path] => {
            assert!(!std::path::Path::new(path).exists(), "{path} exists; not overwriting a release key");
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).expect("random");
            std::fs::write(path, deskmonk::update::hex(&seed)).expect("write key");
            #[cfg(unix)]
            std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).expect("chmod");
            println!("{}", deskmonk::update::hex(SigningKey::from_bytes(&seed).verifying_key().as_bytes()));
        }
        ["sign", key, manifest] => {
            let bytes = std::fs::read(manifest).expect("read manifest");
            serde_json::from_slice::<serde_json::Value>(&bytes).expect("manifest is JSON");
            let sig = deskmonk::update::hex(&key_of(key).sign(&bytes).to_bytes());
            std::fs::write(format!("{manifest}.sig"), sig).expect("write signature");
            println!("signed {manifest}");
        }
        _ => eprintln!("usage: sign_update keygen KEYFILE | sign KEYFILE MANIFEST"),
    }
}
