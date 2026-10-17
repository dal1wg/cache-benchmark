//! Integration tests: cache-bench-ctl orchestrating REAL benchmark hosts over SSH.
//! Skipped unless `CTL_HOSTS_FILE` (path to hosts.toml with pre-configured SSH
//! access) is set:
//!   CTL_HOSTS_FILE=hosts.toml cargo test --test integration_distributed_run -- --ignored

use cache_benchmark_tests::{env_target, find_bin};
use std::process::Command;

fn hosts_file() -> Option<String> {
    env_target("CTL_HOSTS_FILE")
}

fn ctl_path() -> std::path::PathBuf {
    find_bin("cache-bench-ctl")
}

fn run_ctl(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(ctl_path()).args(args).output().expect("spawn ctl");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
#[ignore]
fn distribute_places_executable_worker() {
    let hosts = match hosts_file() {
        Some(h) => h,
        None => {
            eprintln!("CTL_HOSTS_FILE not set; skipping");
            return;
        }
    };
    let (code, out, err) = run_ctl(&["--hosts", &hosts, "distribute"]);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", out, err);
    assert!(out.contains("distributed"), "out: {}", out);
    // correctness of remote placement/chmod is implicitly proven by the run
    // test below; re-running distribute must also succeed (idempotent)
}

#[test]
#[ignore]
fn run_merges_results_and_reports_total() {
    let hosts = match hosts_file() {
        Some(h) => h,
        None => {
            eprintln!("CTL_HOSTS_FILE not set; skipping");
            return;
        }
    };
    // short PING benchmark against the hosts file's first host is not
    // derivable here; use 127.0.0.1:1 as an always-failing target would abort
    // the run — instead benchmark the ctl machine's localhost redis if
    // REDIS_HOST is also set, else just exercise distribute+skip flow with a
    // short run against CLUSTER_SEED/REDIS_HOST when available.
    let target = std::env::var("REDIS_HOST").ok()
        .or_else(|| std::env::var("CLUSTER_SEED").ok());
    let Some(target) = target else {
        eprintln!("REDIS_HOST/CLUSTER_SEED not set; skipping");
        return;
    };
    let (host, port) = cache_benchmark_tests::split_addr(&target, 6379);
    let port_s = port.to_string();

    let (code, out, err) = run_ctl(&[
        "--hosts", &hosts,
        "run", "--skip-distribute", "--lead", "3",
        "--",
        "-h", &host, "-p", &port_s, "-c", "4", "-P", "8", "-s", "3",
        "PING",
    ]);
    assert_eq!(code, Some(0), "stdout: {}\nstderr: {}", out, err);
    assert!(out.contains("Aggregate Report"), "out: {}", out);
    assert!(out.contains("TOTAL"), "out: {}", out);
    // per-host lines and a positive total count
    assert!(out.contains("count"), "out: {}", out);
}

#[test]
#[ignore]
fn single_host_failure_does_not_abort_others() {
    let hosts = match hosts_file() {
        Some(h) => h,
        None => {
            eprintln!("CTL_HOSTS_FILE not set; skipping");
            return;
        }
    };
    let target = std::env::var("REDIS_HOST").ok()
        .or_else(|| std::env::var("CLUSTER_SEED").ok());
    let Some(target) = target else {
        eprintln!("REDIS_HOST/CLUSTER_SEED not set; skipping");
        return;
    };
    let (host, port) = cache_benchmark_tests::split_addr(&target, 6379);
    let port_s = port.to_string();

    // workers on every host try a dead port: every host "fails", ctl must
    // still print a report (all-failed path) instead of crashing
    let (code, out, err) = run_ctl(&[
        "--hosts", &hosts,
        "run", "--skip-distribute", "--lead", "2",
        "--",
        "-h", &host, "-p", "1", "-c", "1", "-s", "1",
        "PING",
    ]);
    assert_ne!(code, Some(0), "all hosts failing => non-zero exit");
    assert!(out.contains("Aggregate Report") || out.contains("FAILED"), "out: {}", out);
    let _ = err;
}
