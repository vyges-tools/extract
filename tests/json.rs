use vyges_extract::rc::NetParasitics;
use vyges_extract::spef::render_json;

#[test]
fn json_summary() {
    let nets = vec![NetParasitics {
        name: "clk".into(),
        raw_name: String::new(),
        pins: vec![("a".into(), "X".into())],
        res_ohm: 10.0,
        cap_ff: 0.5,
    }];
    let j = render_json("d", &nets, &[]);
    assert!(j.contains("\"design\":\"d\""));
    assert!(j.contains("\"nets\":1"));
    assert!(j.contains("\"ground_cap_ff\":0.500000"));
    assert!(j.contains("\"coupling_cap_ff\":0.000000"));
    assert!(j.contains("\"total_cap_ff\":0.500000"));
    assert!(j.contains("\"total_res_ohm\":10.000000"));
    assert!(j.contains("\"couplings\":[]"));
    assert!(j.trim_end().ends_with('}'));
}
