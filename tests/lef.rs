use vyges_extract::lef::Lef;

#[test]
fn parses_routing_widths() {
    let l = Lef::parse(
        "VERSION 5.8 ;\n\
         LAYER li1\n  TYPE ROUTING ;\n  WIDTH 0.17 ;\nEND li1\n\
         LAYER met1\n  TYPE ROUTING ;\n  WIDTH 0.14 ;\nEND met1\n\
         END LIBRARY\n",
    )
    .unwrap();
    assert!((l.width("li1") - 0.17).abs() < 1e-12);
    assert!((l.width("met1") - 0.14).abs() < 1e-12);
    assert_eq!(l.width("nonexistent"), 0.0); // unknown layer -> 0 (centerline fallback)
}
