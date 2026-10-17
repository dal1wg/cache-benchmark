use cache_benchmark::parse_cores_checked;

#[test]
fn single_cores() {
    assert_eq!(parse_cores_checked("0").unwrap(), vec![0]);
    assert_eq!(parse_cores_checked("3").unwrap(), vec![3]);
}

#[test]
fn comma_separated() {
    assert_eq!(parse_cores_checked("0,1,2").unwrap(), vec![0, 1, 2]);
}

#[test]
fn range() {
    assert_eq!(parse_cores_checked("0-3").unwrap(), vec![0, 1, 2, 3]);
}

#[test]
fn mixed_range_and_single() {
    assert_eq!(parse_cores_checked("0-3,7").unwrap(), vec![0, 1, 2, 3, 7]);
}

#[test]
fn dedup_and_sort() {
    assert_eq!(parse_cores_checked("7,0-3,2,7").unwrap(), vec![0, 1, 2, 3, 7]);
}

#[test]
fn whitespace_tolerant() {
    assert_eq!(parse_cores_checked(" 0 , 1 ").unwrap(), vec![0, 1]);
}

#[test]
fn invalid_number() {
    assert!(parse_cores_checked("abc").is_err());
    assert!(parse_cores_checked("0,x").is_err());
}

#[test]
fn invalid_range() {
    assert!(parse_cores_checked("3-0").is_err()); // descending
    assert!(parse_cores_checked("0-1-2").is_err()); // too many parts
}

#[test]
fn empty_spec() {
    assert!(parse_cores_checked("").is_err());
    assert!(parse_cores_checked(",,,,").is_err());
}

#[test]
fn u16_overflow() {
    assert!(parse_cores_checked("70000").is_err());
}
