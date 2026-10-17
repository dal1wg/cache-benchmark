//! Functional tests: cache-bench-ctl CLI behavior.

use cache_benchmark_tests::run_bin;
use std::io::Write;

fn write_temp(name: &str, content: &str) -> String {
    let path = std::env::temp_dir().join(format!("cb-ctl-test-{}-{}", name, std::process::id()));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    path.to_string_lossy().to_string()
}

#[test]
fn no_subcommand_is_error() {
    let hosts = write_temp("hosts.toml", "[[hosts]]\naddress = \"127.0.0.1\"\n");
    let (code, _, err) = run_bin("cache-bench-ctl", &["--hosts", &hosts]);
    assert_ne!(code, Some(0));
    assert!(err.contains("no subcommand"), "err: {}", err);
}

#[test]
fn missing_hosts_file_is_error() {
    let (code, _, err) = run_bin("cache-bench-ctl", &["--hosts", "/nonexistent/hosts.toml", "distribute"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("failed to read hosts file"), "err: {}", err);
}

#[test]
fn invalid_hosts_toml_is_error() {
    let bad = write_temp("bad.toml", "not [valid toml");
    let (code, _, err) = run_bin("cache-bench-ctl", &["--hosts", &bad, "distribute"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("TOML") || err.contains("parse"), "err: {}", err);
}

#[test]
fn hosts_singular_key_is_error() {
    // regression for the [[host]] vs [[hosts]] bug
    let bad = write_temp("singular.toml", "[[host]]\naddress = \"1.2.3.4\"\n");
    let (code, _, err) = run_bin("cache-bench-ctl", &["--hosts", &bad, "distribute"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("hosts"), "err: {}", err);
}

#[test]
fn run_without_worker_args_is_error() {
    let hosts = write_temp("run-hosts.toml", "[[hosts]]\naddress = \"127.0.0.1\"\n");
    let (code, _, err) = run_bin("cache-bench-ctl", &["--hosts", &hosts, "run", "--skip-distribute"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("no worker arguments"), "err: {}", err);
}

#[test]
fn distribute_without_binaries_fails_gracefully() {
    let hosts = write_temp("dist-hosts.toml", "[[hosts]]\naddress = \"127.0.0.1\"\nport = 1\n");
    let empty_dir = std::env::temp_dir().join(format!("cb-ctl-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty_dir).unwrap();
    let dir_str = empty_dir.to_string_lossy().to_string();
    let (code, out, err) = run_bin(
        "cache-bench-ctl",
        &["--hosts", &hosts, "--bin-dir", &dir_str, "distribute"],
    );
    // fails on ssh (port 1 refused) or missing binary — either way non-zero
    assert_ne!(code, Some(0), "out: {} err: {}", out, err);
    let combined = format!("{}{}", out, err);
    assert!(
        combined.contains("distribute failed") || combined.contains("no worker binary"),
        "combined: {}",
        combined
    );
}

#[test]
fn version_flag() {
    let (code, out, _) = run_bin("cache-bench-ctl", &["--version"]);
    assert_eq!(code, Some(0));
    assert!(out.trim().starts_with("cache-bench-ctl"), "out: {}", out);
}
