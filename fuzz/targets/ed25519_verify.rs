#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() >= 96 {
        let pub_key_bytes: [u8; 32] = data[0..32].try_into().unwrap();
        let sig_bytes: [u8; 64] = data[32..96].try_into().unwrap();
        let message = &data[96..];
        if let Ok(pub_key) = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_bytes) {
            let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
            let _ = pub_key.verify_strict(message, &sig);
        }
    }
});
