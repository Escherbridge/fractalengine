use fe_webview::security::is_url_allowed;

#[test]
fn test_localhost_blocked() {
    assert!(!is_url_allowed(&"http://localhost/".parse().unwrap()));
}

#[test]
fn test_127_blocked() {
    assert!(!is_url_allowed(&"http://127.0.0.1/".parse().unwrap()));
}

#[test]
fn test_10_x_x_x_blocked() {
    assert!(!is_url_allowed(&"http://10.0.0.1/".parse().unwrap()));
}

#[test]
fn test_172_16_blocked() {
    assert!(!is_url_allowed(&"http://172.16.0.1/".parse().unwrap()));
}

#[test]
fn test_192_168_blocked() {
    assert!(!is_url_allowed(&"http://192.168.1.1/".parse().unwrap()));
}

#[test]
fn test_public_allowed() {
    assert!(is_url_allowed(&"https://example.com".parse().unwrap()));
}

#[test]
fn test_ipv6_ula_blocked() {
    assert!(!is_url_allowed(&"http://[fc00::1]/".parse().unwrap()));
}
