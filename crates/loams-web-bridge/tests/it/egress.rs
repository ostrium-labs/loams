//! The egress policy: the answer to "may this process become an open proxy?".

use loams_web_bridge::{EgressError, EgressPolicy, HostPattern};

fn url(raw: &str) -> url::Url {
    url::Url::parse(raw).unwrap_or_else(|_| panic!("{raw} parses"))
}

#[test]
fn only_http_and_https_are_navigable() {
    let policy = EgressPolicy::public_web();
    for raw in [
        "file:///etc/passwd",
        "data:text/html,<script>1</script>",
        "javascript:alert(1)",
        "about:blank",
        "ftp://example.com/x",
        "ws://example.com/socket",
    ] {
        let error = policy
            .check(&url(raw))
            .err()
            .unwrap_or_else(|| panic!("{raw} is refused"));
        assert!(
            matches!(error, EgressError::Scheme { .. }),
            "{raw}: {error:?}"
        );
    }
}

#[test]
fn private_loopback_link_local_and_metadata_addresses_are_refused() {
    let policy = EgressPolicy::public_web();
    for raw in [
        "http://127.0.0.1/",
        "http://127.1.2.3:9222/json/version",
        "http://10.0.0.5/",
        "http://172.16.0.1/",
        "http://192.168.1.1/",
        "http://169.254.169.254/latest/meta-data/iam/",
        "http://100.100.100.200/",
        "http://0.0.0.0/",
        "http://[::1]/",
        "http://[fd00::1]/",
        "http://[fe80::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://localhost/",
        "http://service.internal/",
        "http://printer.local/",
        "http://metadata.google.internal/",
    ] {
        assert!(policy.check(&url(raw)).is_err(), "{raw} must be refused");
    }
}

#[test]
fn a_loopback_fixture_is_possible_only_when_asked_for() {
    let mut policy = EgressPolicy::public_web();
    assert!(policy.check(&url("http://127.0.0.1:8080/fixture")).is_err());
    policy.set_allow_private_hosts(true);
    policy
        .check(&url("http://127.0.0.1:8080/fixture"))
        .unwrap_or_else(|error| panic!("a fixture host passes when allowed: {error}"));
}

#[test]
fn an_allow_list_is_exact_unless_a_wildcard_is_written() {
    let policy = EgressPolicy::allow_list(["example.com", "*.assets.example.com"])
        .unwrap_or_else(|error| panic!("{error}"));
    for allowed in [
        "https://example.com/",
        "https://example.com/deep/path?x=1",
        "https://cdn.assets.example.com/x.js",
    ] {
        policy
            .check(&url(allowed))
            .unwrap_or_else(|error| panic!("{allowed} is allowed: {error}"));
    }
    for refused in [
        "https://assets.example.com/",
        "https://www.example.com/",
        "https://example.com.evil.test/",
        "https://evilexample.com/",
        "https://elsewhere.test/",
    ] {
        assert!(policy.check(&url(refused)).is_err(), "{refused} is refused");
    }
}

#[test]
fn a_deny_list_wins_over_an_allow_list() {
    let mut policy = EgressPolicy::allow_list(["*.example.com"]).unwrap_or_else(|_| panic!("ok"));
    policy
        .deny("admin.example.com")
        .unwrap_or_else(|_| panic!("ok"));
    let error = policy
        .check(&url("https://admin.example.com/"))
        .err()
        .unwrap_or_else(|| panic!("refused"));
    assert!(matches!(error, EgressError::DeniedHost { .. }), "{error:?}");
}

#[test]
fn hostname_pattern_rules_are_enforced_locally() {
    assert!(
        HostPattern::parse("https://example.com").is_err(),
        "no scheme"
    );
    assert!(HostPattern::parse("example.com:443").is_err(), "no port");
    assert!(HostPattern::parse("example.com/path").is_err(), "no path");
    assert!(
        HostPattern::parse("*.*.example.com").is_err(),
        "one wildcard"
    );

    let prefix = HostPattern::parse("*example.com").unwrap_or_else(|_| panic!("ok"));
    assert!(prefix.matches("example.com"));
    assert!(
        prefix.matches("evilexample.com"),
        "the apex-matching wildcard"
    );

    let subdomains = HostPattern::parse("*.example.com").unwrap_or_else(|_| panic!("ok"));
    assert!(subdomains.matches("a.b.example.com"));
    assert!(!subdomains.matches("example.com"));
}

#[test]
fn an_allow_list_has_a_ceiling() {
    let hosts: Vec<String> = (0..51)
        .map(|index| format!("host{index}.example.com"))
        .collect();
    assert!(EgressPolicy::allow_list(hosts).is_err(), "51 entries");
    let hosts: Vec<String> = (0..50)
        .map(|index| format!("host{index}.example.com"))
        .collect();
    let policy = EgressPolicy::allow_list(hosts).unwrap_or_else(|_| panic!("50 is fine"));
    assert!(policy.is_allow_listed());
    assert!(!EgressPolicy::public_web().is_allow_listed());
}

#[test]
fn a_page_request_follows_the_navigation_rules_except_for_in_memory_schemes() {
    let policy = EgressPolicy::allow_list(["*.example.com"]).unwrap_or_else(|_| panic!("ok"));
    policy
        .check_request(&url("https://cdn.example.com/app.js"))
        .unwrap_or_else(|error| panic!("{error}"));
    policy
        .check_request(&url("blob:https://app.example.com/1f2e"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        policy
            .check_request(&url("https://tracker.test/p.gif"))
            .is_err()
    );
    assert!(policy.check_request(&url("http://10.0.0.1/admin")).is_err());
}
