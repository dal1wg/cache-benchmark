//! Shared test infrastructure: mock RESP server, binary discovery helpers,
//! and environment-driven skip logic for integration tests.

pub mod mock_resp;

pub use mock_resp::{MockRespConfig, MockRespHandle, start_mock};

use std::path::PathBuf;
use std::process::Command;

/// Locate a workspace binary built by `cargo build` (debug or release).
/// Functional tests spawn these as black-box processes.
pub fn find_bin(name: &str) -> PathBuf {
    // CARGO_BIN_EXE_ is only set for [[bin]] targets of the *same* crate;
    // for cross-crate binaries we search the standard target dirs.
    let exe_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };
    for profile in ["debug", "release"] {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join(profile)
            .join(&exe_name);
        if p.is_file() {
            return p;
        }
    }
    panic!(
        "binary `{}` not found in target/debug or target/release; run `cargo build -p cache-benchmark -p cache-bench-ctl` first",
        name
    );
}

/// Run a workspace binary and return (exit code, stdout, stderr).
pub fn run_bin(name: &str, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(find_bin(name))
        .args(args)
        .output()
        .expect("failed to spawn binary");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Integration-test target: read from env, e.g. `REDIS_HOST=127.0.0.1:6379`.
/// Returns None (test skips) when the env var is unset.
pub fn env_target(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

/// Split "host:port" into (host, port); defaults port when missing.
pub fn split_addr(addr: &str, default_port: u16) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(default_port)),
        None => (addr.to_string(), default_port),
    }
}
