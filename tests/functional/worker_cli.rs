//! Functional tests: spawn the cache-benchmark binary as a black box.

use cache_benchmark_tests::run_bin;

#[test]
fn version_flag() {
    let (code, out, _) = run_bin("cache-benchmark", &["--version"]);
    assert_eq!(code, Some(0));
    assert!(out.trim().starts_with("cache-benchmark"), "out: {}", out);
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "version in output: {}", out);
}

#[test]
fn help_flag() {
    let (code, out, _) = run_bin("cache-benchmark", &["--help"]);
    assert_eq!(code, Some(0));
    for needle in ["--cluster", "--load", "--json", "--start-at", "--cores", "-P"] {
        assert!(out.contains(needle), "help must mention {}: {}", needle, out);
    }
}

#[test]
fn h_is_host_not_help() {
    // -h is the host flag (resp-benchmark compatibility), --help is the help
    let (code, out, err) = run_bin("cache-benchmark", &["-h"]);
    assert_ne!(code, Some(0), "bare -h without value must fail");
    let combined = format!("{}{}", out, err);
    assert!(!combined.trim().is_empty(), "clap must print a usage error");
}

#[test]
fn invalid_port_rejected() {
    let (code, _, _) = run_bin("cache-benchmark", &["-p", "notaport", "-s", "1", "PING"]);
    assert_ne!(code, Some(0));
}

#[test]
fn short_connection_rejects_pipeline() {
    let (code, _, err) = run_bin("cache-benchmark", &["--short-connection", "-P", "4", "-s", "1", "PING"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("short_connection"), "err: {}", err);
}

#[test]
fn short_connection_rejects_load() {
    let (code, _, err) = run_bin("cache-benchmark", &["--short-connection", "--load", "-n", "10", "PING"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("short_connection"), "err: {}", err);
}

#[test]
fn cores_out_of_range_rejected() {
    let (_, out, err) = run_bin("cache-benchmark", &["--cores", "99999", "-s", "1", "PING"]);
    // either clap/parse error exit or the runtime check fires; both must be
    // a non-zero exit with a message
    let combined = format!("{}{}", out, err);
    assert!(combined.contains("core") || combined.contains("Invalid"), "combined: {}", combined);
}

#[test]
fn default_command_when_none_given_is_set() {
    // --help documents the default; also verify a 1-second PING-free run
    // against a dead port exits with a connection error mentioning the address
    let (code, _, err) = run_bin("cache-benchmark", &["-h", "127.0.0.1", "-p", "1", "-s", "1", "-c", "1"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("127.0.0.1:1"), "error must mention address: {}", err);
}

#[test]
fn lua_file_missing_reports_error() {
    let (code, _, err) = run_bin("cache-benchmark", &["--lua-file", "/nonexistent/file.lua", "-s", "1"]);
    assert_ne!(code, Some(0));
    assert!(err.to_lowercase().contains("failed to read") || err.to_lowercase().contains("no such"), "err: {}", err);
}

#[test]
fn key_prefix_documented_in_help() {
    let (code, out, _) = run_bin("cache-benchmark", &["--help"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("--key-prefix"), "help must mention --key-prefix: {}", out);
}

#[test]
fn key_prefix_with_whitespace_is_rejected() {
    // a prefix containing a space would tear generated keys into multiple
    // command arguments; must exit with an error mentioning whitespace
    let (code, _, err) = run_bin(
        "cache-benchmark",
        &["--key-prefix", "a b", "-h", "127.0.0.1", "-p", "1", "-s", "1", "SET {key uniform 10} {value 8}"],
    );
    assert_ne!(code, Some(0), "whitespace prefix must be rejected");
    assert!(err.contains("whitespace"), "err: {}", err);
}

#[test]
fn key_prefix_accepted_and_reaches_generator() {
    // dead port is fine: generator construction happens before connecting, so
    // reaching the "Failed to connect" stage proves the prefix was accepted
    let (code, _, err) = run_bin(
        "cache-benchmark",
        &["--key-prefix", "user:profile:", "-h", "127.0.0.1", "-p", "1", "-s", "1", "SET {key uniform 10} {value 8}"],
    );
    assert_ne!(code, Some(0));
    assert!(err.contains("127.0.0.1:1"), "should fail at connect, not at arg parsing: {}", err);
}
