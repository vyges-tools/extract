use vyges_extract::rc::NetParasitics;
use vyges_extract::spef::render_json;

#[test]
fn json_summary() {
    let nets = vec![NetParasitics {
        name: "clk".into(),
        pins: vec![("a".into(), "X".into())],
        res_ohm: 10.0,
        cap_ff: 0.5,
    }];
    let j = render_json("d", &nets);
    assert!(j.contains("\"design\":\"d\""));
    assert!(j.contains("\"nets\":1"));
    assert!(j.contains("\"cap_ff\":0.500000"));
    assert!(j.contains("\"total_res_ohm\":10.000000"));
    assert!(j.trim_end().ends_with('}'));
}
