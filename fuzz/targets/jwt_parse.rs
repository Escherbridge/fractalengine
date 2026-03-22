#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(token) = std::str::from_utf8(data) {
        let fake_key = ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]);
        if let Ok(key) = fake_key {
            let _ = fe_identity::jwt::verify_session_token(token, &key);
        }
    }
});
