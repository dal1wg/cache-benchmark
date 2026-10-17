use cache_bench_ctl::{find_worker_binary, normalize_arch, shell_escape};
use std::fs;

#[test]
fn arch_aliases() {
    assert_eq!(normalize_arch("x86_64"), Some("x86_64"));
    assert_eq!(normalize_arch("amd64"), Some("x86_64"));
    assert_eq!(normalize_arch("aarch64"), Some("aarch64"));
    assert_eq!(normalize_arch("arm64"), Some("aarch64"));
    assert_eq!(normalize_arch(" x86_64 "), Some("x86_64"), "trims whitespace");
    assert_eq!(normalize_arch("riscv64"), None);
    assert_eq!(normalize_arch(""), None);
}

fn setup_dir(tag: &str, files: &[&str]) -> tempfile_dir::Dir {
    let dir = tempfile_dir::Dir::new(tag);
    for f in files {
        fs::write(dir.path().join(f), b"#!/bin/sh\n").unwrap();
    }
    dir
}

/// Minimal tempdir helper (avoids adding a tempfile dependency).
mod tempfile_dir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new(tag: &str) -> Self {
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("cb-test-{}-{}-{}", tag, std::process::id(), n));
            std::fs::create_dir_all(&p).expect("mkdir");
            Dir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn exact_short_name_wins() {
    let dir = setup_dir("exact", &["cache-benchmark-x86_64", "cache-benchmark-x86_64-unknown-linux-musl"]);
    let found = find_worker_binary(dir.path(), "x86_64").unwrap();
    assert_eq!(found.file_name().unwrap(), "cache-benchmark-x86_64");
}

#[test]
fn legacy_triple_name_accepted() {
    let dir = setup_dir("legacy", &["cache-benchmark-x86_64-unknown-linux-musl"]);
    let found = find_worker_binary(dir.path(), "x86_64").unwrap();
    assert_eq!(found.file_name().unwrap(), "cache-benchmark-x86_64-unknown-linux-musl");
}

#[test]
fn amd64_normalized_to_x86_64() {
    let dir = setup_dir("amd64", &["cache-benchmark-x86_64"]);
    let found = find_worker_binary(dir.path(), "amd64").unwrap();
    assert_eq!(found.file_name().unwrap(), "cache-benchmark-x86_64");
}

#[test]
fn arm64_normalized_to_aarch64() {
    let dir = setup_dir("arm64", &["cache-benchmark-aarch64"]);
    let found = find_worker_binary(dir.path(), "arm64").unwrap();
    assert_eq!(found.file_name().unwrap(), "cache-benchmark-aarch64");
}

#[test]
fn wrong_arch_binary_not_matched() {
    let dir = setup_dir("wrong", &["cache-benchmark-aarch64"]);
    assert!(find_worker_binary(dir.path(), "x86_64").is_err());
}

#[test]
fn empty_dir_is_error() {
    let dir = setup_dir("empty", &[]);
    let err = find_worker_binary(dir.path(), "x86_64").unwrap_err();
    assert!(err.to_string().contains("x86_64"), "error mentions arch: {}", err);
}

#[test]
fn unsupported_arch_is_error() {
    let dir = setup_dir("unsup", &["cache-benchmark-x86_64"]);
    assert!(find_worker_binary(dir.path(), "riscv64").is_err());
}

#[test]
fn directories_are_ignored() {
    let dir = setup_dir("dirs", &[]);
    std::fs::create_dir_all(dir.path().join("cache-benchmark-x86_64")).unwrap();
    assert!(find_worker_binary(dir.path(), "x86_64").is_err(), "dir named like the binary must not match");
}

// ---- shell_escape ----

#[test]
fn escape_plain_word() {
    assert_eq!(shell_escape("hello"), "'hello'");
}

#[test]
fn escape_word_with_spaces() {
    assert_eq!(shell_escape("GET {key uniform 100}"), "'GET {key uniform 100}'");
}

#[test]
fn escape_embedded_single_quote() {
    // 'a'b -> 'a'\''b' : close quote, escaped quote, reopen
    assert_eq!(shell_escape("a'b"), "'a'\\''b'");
}

#[test]
fn escape_empty() {
    assert_eq!(shell_escape(""), "''");
}
