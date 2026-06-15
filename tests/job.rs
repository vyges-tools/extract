use vyges_extract::job::ExtractJob;

#[test]
fn parses_and_defaults() {
    let j = ExtractJob::parse("design: counter\ndef: c.def\nrules: r.rules\n", "work").unwrap();
    assert_eq!(j.design, "counter");
    assert_eq!(j.corner, "typical"); // default
    assert_eq!(j.temp, 25.0); // default
    assert_eq!(j.resolve("c.def"), "work/c.def");
}

#[test]
fn missing_required_keys_error() {
    assert!(ExtractJob::parse("design: x\n", ".").is_err()); // no def/rules
}
