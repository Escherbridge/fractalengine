use fe_webview::security::is_url_allowed;

#[test]
fn test_localhost_blocked() {
    for raw in [
        "http://localhost/",
        "http://LOCALHOST./",
        "http://a.localhost/",
        "http://nested.a.localhost./",
        "http://localhost.localdomain/",
        "http://localhost6/",
        "http://localhost6.localdomain6/",
        "http://ip6-localhost/",
        "http://ip6-loopback/",
    ] {
        assert!(
            !is_url_allowed(&raw.parse().unwrap()),
            "loopback alias: {raw}"
        );
    }
}

#[test]
fn test_127_blocked() {
    for raw in [
        "http://127.0.0.1/",
        "http://127.0.0.0/",
        "http://127.42.80.3/",
        "http://127.255.255.255/",
        "http://127.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://0177.0.0.1/",
        "http://[::1]/",
        "http://[0:0:0:0:0:0:0:1]/",
        "http://[::ffff:127.2.3.4]/",
        "http://[::ffff:7f00:1]/",
        "http://[::127.0.0.1]/",
    ] {
        assert!(
            !is_url_allowed(&raw.parse().unwrap()),
            "canonical loopback: {raw}"
        );
    }
}

#[test]
fn test_10_x_x_x_blocked() {
    for raw in [
        "http://10.0.0.1/",
        "http://[::ffff:10.0.0.1]/",
        "http://[::10.0.0.1]/",
        "http://169.254.3.2/",
        "http://[::ffff:169.254.3.2]/",
        "http://[::ffff:0.0.0.0]/",
    ] {
        assert!(
            !is_url_allowed(&raw.parse().unwrap()),
            "configured private/unspecified denial: {raw}"
        );
    }
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
    for raw in [
        "https://example.com",
        "http://8.8.8.8/",
        "http://[::ffff:8.8.8.8]/",
        "https://[2001:4860:4860::8888]/",
        "https://localhost.example.com/",
        "https://notlocalhost.example/",
        "https://localhost6.example/",
        "http://128.0.0.1/",
    ] {
        assert!(
            is_url_allowed(&raw.parse().unwrap()),
            "public/default allowance: {raw}"
        );
    }
}

#[test]
fn test_ipv6_ula_blocked() {
    assert!(!is_url_allowed(&"http://[fc00::1]/".parse().unwrap()));
}
