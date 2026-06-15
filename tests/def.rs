use vyges_extract::def::parse;

const DEF: &str = "\
UNITS DISTANCE MICRONS 1000 ;
NETS 2 ;
- clk ( clkbuf X ) ( ff0 CLK ) ( ff1 CLK )
  + ROUTED met1 ( 1000 2000 ) ( 1000 5000 )
    NEW met2 ( 1000 5000 ) M1M2_VIA ( 4000 5000 ) ;
- n0 ( u0 Y ) ( u1 A )
  + ROUTED met1 ( 2000 2000 ) ( 2000 2800 ) ;
END NETS
";

#[test]
fn parses_scale_and_nets() {
    let d = parse(DEF).unwrap();
    assert_eq!(d.units_per_um, 1000.0);
    assert_eq!(d.nets.len(), 2);
}

#[test]
fn parses_pins_segments_vias() {
    let d = parse(DEF).unwrap();
    let clk = &d.nets[0];
    assert_eq!(clk.name, "clk");
    assert_eq!(clk.pins.len(), 3);
    assert_eq!(clk.pins[0], ("clkbuf".into(), "X".into()));
    // met1 3um + met2 3um (the via token does not break the run's geometry)
    assert_eq!(clk.segments.len(), 2);
    assert_eq!(clk.segments[0].layer, "met1");
    assert!((clk.segments[0].len_um() - 3.0).abs() < 1e-9);
    assert_eq!(clk.segments[1].layer, "met2");
    assert!((clk.segments[1].len_um() - 3.0).abs() < 1e-9);
    assert_eq!(clk.vias, 1);
}

#[test]
fn star_coordinate_reuses_previous() {
    let d = parse(
        "UNITS DISTANCE MICRONS 1000 ;
         NETS 1 ;
         - a ( i p )
           + ROUTED met1 ( 0 0 ) ( * 4000 ) ( 2000 * ) ;
         END NETS",
    )
    .unwrap();
    let a = &d.nets[0];
    // (0,0)->(0,4000)=4um, (0,4000)->(2000,4000)=2um
    assert_eq!(a.segments.len(), 2);
    assert!((a.segments[0].len_um() - 4.0).abs() < 1e-9);
    assert!((a.segments[1].len_um() - 2.0).abs() < 1e-9);
}
