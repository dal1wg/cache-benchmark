use cache_bench_ctl::hosts::{parse_hosts, Host};

#[test]
fn parse_minimal_entry() {
    let hosts = parse_hosts("[[hosts]]\naddress = \"10.0.0.1\"\n").unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].address, "10.0.0.1");
}

#[test]
fn parse_defaults() {
    let hosts = parse_hosts("[[hosts]]\naddress = \"10.0.0.1\"\n").unwrap();
    let h: &Host = &hosts[0];
    assert_eq!(h.port, 22);
    assert_eq!(h.user, "root");
    assert_eq!(h.remote_dir, "/tmp/cache-benchmark");
    assert_eq!(h.arch, "");
    assert_eq!(h.password, "");
}

#[test]
fn parse_all_fields() {
    let toml = r#"
[[hosts]]
address = "10.0.0.2"
port = 2222
user = "admin"
arch = "aarch64"
remote_dir = "/data/bench"
password = "s3cret"
"#;
    let hosts = parse_hosts(toml).unwrap();
    assert_eq!(hosts[0].port, 2222);
    assert_eq!(hosts[0].user, "admin");
    assert_eq!(hosts[0].arch, "aarch64");
    assert_eq!(hosts[0].remote_dir, "/data/bench");
    assert_eq!(hosts[0].password, "s3cret");
}

#[test]
fn parse_multiple_entries() {
    let toml = r#"
[[hosts]]
address = "10.0.0.1"

[[hosts]]
address = "10.0.0.2"

[[hosts]]
address = "10.0.0.3"
"#;
    let hosts = parse_hosts(toml).unwrap();
    assert_eq!(hosts.len(), 3);
    let addrs: Vec<&str> = hosts.iter().map(|h| h.address.as_str()).collect();
    assert_eq!(addrs, vec!["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
}

#[test]
fn missing_address_is_error() {
    let toml = r#"
[[hosts]]
port = 22
"#;
    assert!(parse_hosts(toml).is_err());
}

#[test]
fn no_hosts_is_error() {
    assert!(parse_hosts("other = 1\n").is_err());
    assert!(parse_hosts("").is_err());
}

#[test]
fn invalid_toml_is_error() {
    assert!(parse_hosts("[[hosts\naddress=x\n").is_err());
}

#[test]
fn hosts_key_must_be_plural() {
    // regression: an early bug renamed the field to singular `host`
    assert!(parse_hosts("[[host]]\naddress = \"1.2.3.4\"\n").is_err());
}
