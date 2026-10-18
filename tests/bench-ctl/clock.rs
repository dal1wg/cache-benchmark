use cache_bench_ctl::{compensated_start_at, offset_from_probe, parse_remote_timestamp};

// ---- parse_remote_timestamp ----

#[test]
fn parses_gnu_fractional_output() {
    let v = parse_remote_timestamp("1792302394.123456789\n").unwrap();
    assert!((v - 1792302394.123456789).abs() < 1e-6);
}

#[test]
fn parses_plain_seconds() {
    let v = parse_remote_timestamp("1792302394").unwrap();
    assert_eq!(v, 1792302394.0);
}

#[test]
fn parses_with_surrounding_whitespace() {
    let v = parse_remote_timestamp("  1792302394.5  \n").unwrap();
    assert!((v - 1792302394.5).abs() < 1e-9);
}

#[test]
fn rejects_garbage() {
    assert!(parse_remote_timestamp("").is_none());
    assert!(parse_remote_timestamp("not-a-date").is_none());
    assert!(parse_remote_timestamp("-123.0").is_none());
    assert!(parse_remote_timestamp("nan").is_none());
    assert!(parse_remote_timestamp("inf").is_none());
}

// ---- offset_from_probe (NTP midpoint) ----

#[test]
fn zero_rtt_zero_offset() {
    // remote timestamp equals local midpoint: offset 0
    let (off, rtt) = offset_from_probe(1000.0, 1000.0, 1000.0);
    assert!(off.abs() < 1e-12);
    assert!(rtt.abs() < 1e-12);
}

#[test]
fn offset_uses_midpoint_of_probe_window() {
    // exec took 100ms; remote clock read 1000.200 at the midpoint (t=1000.150)
    // => remote is 50ms ahead
    let (off, rtt) = offset_from_probe(1000.100, 1000.200, 1000.200);
    assert!((off - 0.050).abs() < 1e-9, "offset: {}", off);
    assert!((rtt - 100.0).abs() < 1e-9);
}

#[test]
fn negative_offset_when_remote_behind() {
    // remote reads 999.900 at local midpoint 1000.150 => -250ms
    let (off, _) = offset_from_probe(1000.100, 1000.200, 999.900);
    assert!((off - (-0.250)).abs() < 1e-9);
}

#[test]
fn rtt_reported_in_millis() {
    let (_, rtt) = offset_from_probe(0.0, 0.25, 0.125);
    assert!((rtt - 250.0).abs() < 1e-9);
}

// ---- compensated_start_at ----

#[test]
fn no_offset_returns_reference() {
    assert!((compensated_start_at(1792300000.0, 0.0) - 1792300000.0).abs() < 1e-9);
}

#[test]
fn remote_ahead_adds_offset() {
    // remote = local + 20s: worker must be told a timestamp 20s larger
    let ts = compensated_start_at(1000.0, 20.0);
    assert!((ts - 1020.0).abs() < 1e-9);
}

#[test]
fn remote_behind_subtracts_offset() {
    // remote = local - 14s
    let ts = compensated_start_at(1000.0, -14.0);
    assert!((ts - 986.0).abs() < 1e-9);
}

#[test]
fn roundtrip_with_probe_math() {
    // a probe observed remote=1200.0 at local midpoint 1000.0 (offset +200s):
    // for a true joint start at local 2000.0 the worker deadline must be 2200.0
    let (off, _) = offset_from_probe(999.9, 1000.1, 1200.0);
    let ts = compensated_start_at(2000.0, off);
    assert!((ts - 2200.0).abs() < 1e-6);
}

#[test]
fn degenerate_inputs_fall_back_to_reference() {
    // NaN/inf/negative results must never reach the worker command line
    let r = 1792300000.0;
    assert!((compensated_start_at(r, f64::NAN) - r).abs() < 1e-9);
    assert!((compensated_start_at(r, f64::INFINITY) - r).abs() < 1e-9);
    // offset larger than the reference (clock epoch nonsense): clamp to >0
    assert!(compensated_start_at(r, -r * 2.0) > 0.0);
}

#[test]
fn millisecond_precision_preserved() {
    let ts = compensated_start_at(1792300000.0, -0.123);
    let frac = (ts * 1000.0).round() % 1000.0;
    assert!((frac - 877.0).abs() < 1e-6, "frac: {}", frac);
}
