use cache_benchmark::command::{CommandGenerator, PlaceholderEnum, PlaceholderGenerator, PlaceholderValue};

/// Extract args of a generated redis::Cmd as strings.
fn cmd_args(cmd: &redis::Cmd) -> Vec<String> {
    cmd.args_iter()
        .map(|a| match a {
            redis::Arg::Simple(s) => String::from_utf8_lossy(s).to_string(),
            redis::Arg::Cursor => "Cursor".to_string(),
        })
        .collect()
}

#[test]
fn set_command_args() {
    let mut g = PlaceholderGenerator::try_new("SET {key sequence 100} {value 8}").unwrap();
    let args = cmd_args(&g.gen_cmd());
    assert_eq!(args[0], "SET");
    assert!(args[1].starts_with("key_"), "key placeholder format: {}", args[1]);
    assert_eq!(args[2].len(), 8, "value placeholder length");
}

#[test]
fn plain_string_command() {
    let mut g = PlaceholderGenerator::try_new("PING").unwrap();
    assert_eq!(cmd_args(&g.gen_cmd()), vec!["PING"]);
}

#[test]
fn key_uniform_within_range() {
    let mut g = PlaceholderGenerator::try_new("GET {key uniform 10}").unwrap();
    for _ in 0..200 {
        let args = cmd_args(&g.gen_cmd());
        let num: u64 = args[1].trim_start_matches("key_").parse().unwrap();
        assert!(num < 10, "uniform key out of range: {}", num);
    }
}

#[test]
fn key_sequence_is_ordered_and_wraps() {
    let mut g = PlaceholderGenerator::try_new("GET {key sequence 3}").unwrap();
    let expected = ["key_0000000000", "key_0000000001", "key_0000000002", "key_0000000000"];
    for want in expected {
        let args = cmd_args(&g.gen_cmd());
        assert_eq!(args[1], want);
    }
}

#[test]
fn key_zipfian_within_range() {
    // rand_distr::Zipf samples in [1, N] INCLUSIVE (inherited resp-benchmark
    // semantics: zipfian keys may reach key_N, one past uniform's [0, N))
    let mut g = PlaceholderGenerator::try_new("GET {key zipfian 100}").unwrap();
    for _ in 0..300 {
        let args = cmd_args(&g.gen_cmd());
        let num: u64 = args[1].trim_start_matches("key_").parse().unwrap();
        assert!(num >= 1 && num <= 100, "zipfian key out of [1,100]: {}", num);
    }
}

#[test]
fn rand_within_range() {
    let mut g = PlaceholderGenerator::try_new("INCR counter_{rand 5}").unwrap();
    for _ in 0..100 {
        let args = cmd_args(&g.gen_cmd());
        let arg = args.last().unwrap();
        let num: u64 = arg.trim_start_matches("counter_").parse().unwrap();
        assert!(num < 5);
    }
}

#[test]
fn range_placeholder_two_args_clamped() {
    // {range N W}: start in [0, N), end = start+W clamped to N-1
    let mut g = PlaceholderGenerator::try_new("LRANGE k {range 100 10}").unwrap();
    for _ in 0..100 {
        let args = cmd_args(&g.gen_cmd());
        assert!(args.len() == 4, "LRANGE needs 4 args: {:?}", args);
        let start: u64 = args[2].parse().unwrap();
        let end: u64 = args[3].parse().unwrap();
        assert!(start < 100);
        assert_eq!(end, (start + 10).min(99));
    }
}

#[test]
fn range_placeholder_regression_no_concat() {
    // regression: multi-token placeholder output used to be concatenated onto
    // the previous word without a separator ("96" + "99" -> "9699")
    let mut g = PlaceholderGenerator::try_new("GET {range 50 5}").unwrap();
    let args = cmd_args(&g.gen_cmd());
    assert_eq!(args.len(), 3, "GET + two range numbers: {:?}", args);
    let start: u64 = args[1].parse().unwrap();
    let end: u64 = args[2].parse().unwrap();
    assert!(start < 50);
    assert_eq!(end, (start + 5).min(49));
}

#[test]
fn value_lengths() {
    for size in [1u64, 8, 64, 1024] {
        let v = PlaceholderValue::new(size).generate();
        assert_eq!(v.len(), size as usize);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}

#[test]
fn gen_cmd_with_lock_matches_gen_cmd_shape() {
    let mut a = PlaceholderGenerator::try_new("SET {key uniform 100} {value 8}").unwrap();
    let mut b = PlaceholderGenerator::try_new("SET {key uniform 100} {value 8}").unwrap();
    assert_eq!(cmd_args(&a.gen_cmd()).len(), cmd_args(&b.gen_cmd_with_lock()).len());
}

#[test]
fn clone_box_produces_same_command_string() {
    let g = PlaceholderGenerator::try_new("GET {key uniform 5}").unwrap();
    assert_eq!(g.to_string(), "GET {key uniform 5}");
    let cloned = g.clone_box();
    assert_eq!(cloned.to_string(), "GET {key uniform 5}");
}

// ---- invalid inputs (try_new returns Err, no exit/panic) ----

#[test]
fn invalid_placeholder_name() {
    let err = PlaceholderEnum::try_new("bogus 1").unwrap_err();
    assert!(err.contains("Invalid placeholder"), "err: {}", err);
}

#[test]
fn wrong_arg_count() {
    assert!(PlaceholderEnum::try_new("key uniform").is_err());
    assert!(PlaceholderEnum::try_new("value").is_err());
    assert!(PlaceholderEnum::try_new("rand").is_err());
    assert!(PlaceholderEnum::try_new("range 5").is_err());
}

#[test]
fn non_numeric_args() {
    assert!(PlaceholderEnum::try_new("key uniform abc").is_err());
    assert!(PlaceholderEnum::try_new("value abc").is_err());
    assert!(PlaceholderEnum::try_new("rand xyz").is_err());
    assert!(PlaceholderEnum::try_new("range a b").is_err());
}

#[test]
fn unknown_distribution() {
    let err = PlaceholderEnum::try_new("key gaussian 100").unwrap_err();
    assert!(err.contains("Unknown distribution"), "err: {}", err);
}

#[test]
fn empty_placeholder() {
    assert!(PlaceholderEnum::try_new("   ").is_err());
}

#[test]
fn generator_unclosed_brace_is_error() {
    // nom's all_consuming should reject a dangling '{'
    assert!(PlaceholderGenerator::try_new("SET {key uniform 10").is_err());
}
