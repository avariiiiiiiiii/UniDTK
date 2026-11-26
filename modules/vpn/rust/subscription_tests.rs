use super::{header_text, normalize_link, redact_urls, split_placeholders};
use crate::parse_subscription;

#[test]
fn a_plain_address_passes_and_an_app_link_is_unwrapped() {
    assert_eq!(normalize_link("  https://sub.example.com/a/b?x=1 ").unwrap(), "https://sub.example.com/a/b?x=1");
    assert_eq!(normalize_link("").unwrap(), "");
    // Live, 2026-11-26: the user pasted Happ's "add" link and the fetch failed with
    // `Unknown Scheme: unknown scheme 'happ'`.
    assert_eq!(normalize_link("happ://add/https://zestful.example/AbC+d").unwrap(), "https://zestful.example/AbC+d");
    assert_eq!(normalize_link("incy://add/https://a.example/s").unwrap(), "https://a.example/s");
    assert_eq!(normalize_link("v2raytun://import/https://a.example/s#My%20VPN").unwrap(), "https://a.example/s");
    assert_eq!(normalize_link("hiddify://import/https://a.example/s").unwrap(), "https://a.example/s");
    assert_eq!(
        normalize_link("clash://install-config?url=https%3A%2F%2Fa.example%2Fs%3Ftoken%3Dx%2By&name=n").unwrap(),
        "https://a.example/s?token=x+y"
    );
    assert_eq!(
        normalize_link("sing-box://import-remote-profile?url=https%3A%2F%2Fa.example%2Fs#Name").unwrap(),
        "https://a.example/s"
    );
}

#[test]
fn a_plus_in_a_token_stays_a_plus() {
    assert_eq!(normalize_link("happ://add/https://a.example/t+k%2Bz").unwrap(), "https://a.example/t+k%2Bz");
    assert_eq!(normalize_link("clash://install-config?url=https://a.example/t+k").unwrap(), "https://a.example/t+k");
}

#[test]
fn what_cannot_be_fetched_is_refused_with_a_reason_and_without_the_link() {
    let secret = "SeCrEtToKeN";
    for link in [
        format!("happ://crypt4/{secret}"),
        format!("happ://crypt5/{secret}"),
        format!("incy://crypt1/{secret}"),
        format!("happ://add/happ://add/https://a.example/{secret}"),
        format!("https://user:{secret}@a.example/s"),
        format!("https://127.0.0.1/{secret}"),
        format!("https://localhost/{secret}"),
        format!("http://[::1]/{secret}"),
        format!("https://169.254.1.1/{secret}"),
        format!("ftp://a.example/{secret}"),
        format!("just {secret}"),
        format!("happ://add/{secret}"),
    ] {
        let error = normalize_link(&link).unwrap_err();
        assert!(!error.contains(secret), "{link}: {error}");
        assert!(!error.is_empty());
    }
    assert!(normalize_link("happ://crypt4/x").unwrap_err().contains("Зашифрованн"));
    // A home server on the local network is a real subscription source.
    assert_eq!(normalize_link("http://192.168.31.5:2096/sub/x").unwrap(), "http://192.168.31.5:2096/sub/x");
}

#[test]
fn an_error_keeps_the_host_and_drops_the_token_and_the_user() {
    let message = "subscription request failed: https://sub.example.com/api/v1/TOKEN123?client=x: Connection Failed";
    let redacted = redact_urls(message);
    assert!(!redacted.contains("TOKEN123") && !redacted.contains("client=x"), "{redacted}");
    assert!(redacted.contains("https://sub.example.com/…"), "{redacted}");
    let with_user = redact_urls("GET https://alice:pw123@sub.example.com: refused");
    assert!(!with_user.contains("alice") && !with_user.contains("pw123"), "{with_user}");
    assert!(with_user.contains("https://sub.example.com"), "{with_user}");
    assert_eq!(redact_urls("no url here"), "no url here");
    assert_eq!(redact_urls("host https://only.example.com ok"), "host https://only.example.com ok");
}

#[test]
fn a_header_in_base64_is_read_as_text() {
    assert_eq!(header_text("base64:0J/RgNC40LLQtdGC"), "Привет");
    assert_eq!(header_text("  plain title "), "plain title");
}

// Live, 2026-11-26: without a device id the provider sent three entries at 0.0.0.0 whose
// names are its message, and the picker showed them as servers.
#[test]
fn placeholders_become_comments_and_never_servers() {
    let body = "\
vless://00000000-0000-0000-0000-000000000000@0.0.0.0:1?security=none#%E2%9D%8C%20%D0%92%D0%B0%D1%88%D0%B5%20%D0%BF%D1%80%D0%B8%D0%BB%D0%BE%D0%B6%D0%B5%D0%BD%D0%B8%D0%B5\n\
vless://id@real.example:443?security=tls&sni=real.example#Real\n\
vless://11111111-2222-3333-4444-555555555555@0.0.0.0:443#Notice%20two\n\
trojan://00000000-0000-0000-0000-000000000000@x.example:443#Zero%20password";
    let (servers, skipped) = parse_subscription(body);
    assert_eq!(skipped, 0);
    let (servers, comments) = split_placeholders(servers);
    assert_eq!(servers.iter().map(|s| s.tag.as_str()).collect::<Vec<_>>(), ["Real"]);
    assert_eq!(comments, ["❌ Ваше приложение", "Notice two", "Zero password"]);
}
