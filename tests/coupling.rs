use std::collections::BTreeMap;

use vyges_extract::coupling::{extract_coupling, extract_coupling_blocked, extract_coupling_capped};
use vyges_extract::def::{DefNet, Segment};
use vyges_extract::rules::RcRules;

fn no_widths() -> BTreeMap<String, f64> {
    BTreeMap::new()
}

fn hnet(name: &str, x0: f64, x1: f64, y: f64) -> DefNet {
    DefNet {
        name: name.into(),
        pins: vec![],
        segments: vec![Segment::wire("met1", x0, y, x1, y)],
        vias: 0,
        via_points: Vec::new(),
    }
}

#[test]
fn parallel_segments_couple() {
    // met1: coupling 0.1 fF/um at s_ref 0.5um (cols: res cap coupling s_ref)
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    // a: x[0,3] @ y0 ; b: x[1,5] @ y0.5  -> overlap_x = 2.0, gap = 0.5 (== s_ref -> factor 1.0)
    let cc = extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), hnet("b", 1.0, 5.0, 0.5)],
        &rules,
        &no_widths(),
        &no_widths(),
    );
    assert_eq!(cc.len(), 1);
    assert_eq!((cc[0].a.as_str(), cc[0].b.as_str()), ("a", "b"));
    // Cc = 0.1 * 2.0 * (0.5 / max(0.5,0.5)) = 0.2
    assert!((cc[0].cap_ff - 0.2).abs() < 1e-9, "cc={}", cc[0].cap_ff);
}

#[test]
fn wider_gap_couples_less() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    let near = extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 0.5)],
        &rules,
        &no_widths(),
        &no_widths(),
    );
    let far = extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.5)],
        &rules,
        &no_widths(),
        &no_widths(),
    );
    assert!(far[0].cap_ff < near[0].cap_ff, "1/gap falloff");
}

#[test]
fn beyond_cutoff_no_coupling() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 1.0\n").unwrap();
    // gap 1.5 > cutoff 1.0
    let cc = extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.5)],
        &rules,
        &no_widths(),
        &no_widths(),
    );
    assert!(cc.is_empty());
}

#[test]
fn lef_width_uses_edge_gap() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\n").unwrap();
    let mut widths = BTreeMap::new();
    widths.insert("met1".to_string(), 0.4); // wide wires -> smaller edge gap
                                            // centerline gap 1.0 ; edge gap = 1.0 - 0.4 = 0.6 ; overlap 3.0 ; s_ref 0.5
    let with_w = extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.0)],
        &rules,
        &widths,
        &no_widths(),
    );
    let plain = extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), hnet("b", 0.0, 3.0, 1.0)],
        &rules,
        &no_widths(),
        &no_widths(),
    );
    // edge gap (0.6) < centerline (1.0) -> stronger coupling
    assert!(with_w[0].cap_ff > plain[0].cap_ff);
    // Cc = 0.1 * 3.0 * (0.5 / max(0.6,0.5)) = 0.3 * (0.5/0.6) = 0.25
    assert!(
        (with_w[0].cap_ff - 0.25).abs() < 1e-9,
        "cc={}",
        with_w[0].cap_ff
    );
}

#[test]
fn different_layers_dont_couple_laterally() {
    // no `interlayer` rule -> different layers contribute nothing
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\nmet2 0.1 0.05 0.1 0.5\n").unwrap();
    let mut b = hnet("b", 0.0, 3.0, 0.5);
    b.segments[0].layer = "met2".into();
    assert!(extract_coupling(
        &[hnet("a", 0.0, 3.0, 0.0), b],
        &rules,
        &no_widths(),
        &no_widths()
    )
    .is_empty());
}

fn seg(layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> DefNet {
    DefNet {
        name: layer.into(),
        pins: vec![],
        segments: vec![Segment::wire(layer, x0, y0, x1, y1)],
        vias: 0,
        via_points: Vec::new(),
    }
}

#[test]
fn interlayer_crossover_couples_by_area() {
    let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\ninterlayer met1 met2 0.02\n")
        .unwrap();
    let mut widths = BTreeMap::new();
    widths.insert("met1".to_string(), 0.4);
    widths.insert("met2".to_string(), 0.5);
    // met1 horizontal x[0,4] @ y2 (footprint y[1.8,2.2]); met2 vertical x2 y[0,4] (x[1.75,2.25])
    let mut a = seg("met1", 0.0, 2.0, 4.0, 2.0);
    a.name = "a".into();
    let mut b = seg("met2", 2.0, 0.0, 2.0, 4.0);
    b.name = "b".into();
    let cc = extract_coupling(&[a, b], &rules, &widths, &no_widths());
    assert_eq!(cc.len(), 1);
    // overlap area = 0.5 (x) * 0.4 (y) = 0.2 ; Cc = 0.02 * 0.2 = 0.004
    assert!((cc[0].cap_ff - 0.004).abs() < 1e-9, "cc={}", cc[0].cap_ff);
}

#[test]
fn interlayer_needs_widths() {
    let rules = RcRules::parse("met1 0.1 0.05 0.0\nmet2 0.1 0.05 0.0\ninterlayer met1 met2 0.02\n")
        .unwrap();
    let mut a = seg("met1", 0.0, 2.0, 4.0, 2.0);
    a.name = "a".into();
    let mut b = seg("met2", 2.0, 0.0, 2.0, 4.0);
    b.name = "b".into();
    // zero-width footprints -> no overlap area -> no coupling
    assert!(extract_coupling(&[a, b], &rules, &no_widths(), &no_widths()).is_empty());
}

#[test]
fn spatial_index_agrees_with_isolated_pairs_and_respects_line_of_sight() {
    // A deterministic grid of horizontal wires on two layers at varied offsets.
    //
    // Two properties, and the second is why this test changed shape. Coupling is now
    // line-of-sight limited — a wire couples to its nearest visible neighbour per side, not
    // to everything inside the cutoff — so a pair extracted IN ISOLATION is no longer the
    // same as that pair extracted IN CONTEXT. Context is the point: an isolated pair has
    // nothing between it, so the isolated result is an upper bound, not an oracle.
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\nmet2 0.12 0.05 0.08 0.5\n").unwrap();
    let mut nets: Vec<DefNet> = Vec::new();
    for i in 0..60u32 {
        let layer = if i % 2 == 0 { "met1" } else { "met2" };
        let y = (i as f64) * 0.31; // some pairs within cutoff, some not
        let x0 = (i % 7) as f64 * 0.2;
        let mut n = hnet(&format!("n{i}"), x0, x0 + 5.0, y);
        n.segments[0].layer = layer.into();
        n.name = format!("n{i}");
        nets.push(n);
    }
    let got = extract_coupling(&nets, &rules, &no_widths(), &no_widths());

    // Upper bound: every pair, extracted with nothing between it.
    let mut isolated: std::collections::BTreeMap<(String, String), f64> = Default::default();
    for i in 0..nets.len() {
        for j in (i + 1)..nets.len() {
            for c in extract_coupling(
                &[nets[i].clone(), nets[j].clone()],
                &rules,
                &no_widths(),
                &no_widths(),
            ) {
                isolated.insert((c.a, c.b), c.cap_ff);
            }
        }
    }

    // 1. The index invents nothing, and context can only ever REMOVE coupling: a pair
    //    extracted in isolation is the upper bound on the same pair extracted in context.
    for c in &got {
        let w = isolated
            .get(&(c.a.clone(), c.b.clone()))
            .unwrap_or_else(|| panic!("indexed pair {}/{} absent from the isolated set", c.a, c.b));
        assert!(
            c.cap_ff <= w + 1e-12,
            "{}/{} reports {} in context, above its isolated {}",
            c.a,
            c.b,
            c.cap_ff,
            w
        );
    }

    // 2. Line of sight actually bites: the wires are stacked 0.62 apart per layer and the
    //    cutoff reaches several of them, so without occlusion each would couple to every
    //    neighbour in reach rather than to what it can still see.
    assert!(
        got.len() < isolated.len(),
        "occlusion should remove hidden pairs: {} indexed vs {} isolated",
        got.len(),
        isolated.len()
    );

    // 3. Shadowing is by COVERAGE, not by rank: because these wires start at staggered x,
    //    a nearer neighbour spans only part of the parallel run and the remainder still
    //    reaches past it. So some pair must survive with strictly less than its isolated
    //    cap — a rank rule would either keep a pair whole or drop it outright.
    let partial = got.iter().any(|c| {
        isolated
            .get(&(c.a.clone(), c.b.clone()))
            .is_some_and(|w| c.cap_ff < w - 1e-9)
    });
    assert!(
        partial,
        "no pair was partially shadowed — coverage is not being applied"
    );
}

/// A neighbour hides the run it spans and no more. The reference carries the uncovered
/// remainder outward to the next track (`Track::findOverlap`'s len1/len3 split), so a
/// wire beyond a SHORT blocker still couples — over what is left of the run, at its own
/// wider gap. The rank rule this replaced dropped that pair entirely.
#[test]
fn a_short_neighbour_shadows_only_the_run_it_spans() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 2.0\n").unwrap();
    let nets = [
        hnet("a", 0.0, 10.0, 0.0),  // source
        hnet("b", 0.0, 4.0, 0.5),   // covers only x[0,4] of it
        hnet("c", 0.0, 10.0, 1.0),  // visible over the remaining x[4,10]
    ];
    let cc = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    let find = |x: &str, y: &str| {
        cc.iter()
            .find(|c| c.a == x && c.b == y)
            .unwrap_or_else(|| panic!("{x}/{y} missing from {cc:?}"))
            .cap_ff
    };
    // a–b: the full 4.0 of b's run, at gap 0.5 (== s_ref, factor 1.0)
    assert!((find("a", "b") - 0.4).abs() < 1e-9, "a/b = {}", find("a", "b"));
    // a–c: the 6.0 b does not span, at gap 1.0 -> 0.1 * 6 * (0.5/1.0)
    assert!((find("a", "c") - 0.3).abs() < 1e-9, "a/c = {}", find("a", "c"));
    // b–c: b's own view upward, its whole 4.0 run at gap 0.5
    assert!((find("b", "c") - 0.4).abs() < 1e-9, "b/c = {}", find("b", "c"));
}

/// The other half of the same rule: a neighbour that spans the WHOLE parallel run leaves
/// nothing uncovered, so the wire behind it is not coupled to at all.
#[test]
fn a_covering_neighbour_hides_what_is_behind_it() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 2.0\n").unwrap();
    let nets = [
        hnet("a", 0.0, 10.0, 0.0),
        hnet("b", -1.0, 11.0, 0.5), // spans past both ends of a
        hnet("c", 0.0, 10.0, 1.0),
    ];
    let cc = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    assert!(
        !cc.iter().any(|c| c.a == "a" && c.b == "c"),
        "a couples through a wire that fully covers it: {cc:?}"
    );
    assert!(cc.iter().any(|c| c.a == "a" && c.b == "b"));
    assert!(cc.iter().any(|c| c.b == "c" && c.a == "b"));
}

/// **The walk is one-sided, so prove it is unbiased.**
///
/// Each pair is booked once, by the segment below it looking up — the reference does the same
/// (its downward pass runs with `handleEmptyOnly`, which suppresses coupling). That is only
/// sound if the answer does not depend on which way is "up". Mirror the whole design and every
/// coupling value must be bit-identical: a one-sided walk that leaked bias would report
/// different numbers for the same geometry seen upside down.
///
/// This is the cheapest possible check on the riskiest line of the change, and no correlation
/// run can substitute for it — a systematic bias would just be absorbed by the fitted
/// coefficient, exactly as the earlier errors were.
#[test]
fn coupling_is_invariant_under_mirroring() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\nmet2 0.12 0.05 0.08 0.5\ncouple_cutoff 2.0\n")
        .unwrap();
    // A deterministic tangle: staggered run extents so shadowing is partial, both orientations,
    // both layers, and several nets multi-segment.
    let mut nets: Vec<DefNet> = Vec::new();
    for i in 0..40u32 {
        let f = i as f64;
        let mut n = DefNet {
            name: format!("n{i}"),
            pins: vec![],
            segments: vec![],
            vias: 0,
            via_points: Vec::new(),
        };
        let layer = if i % 3 == 0 { "met2" } else { "met1" };
        if i % 2 == 0 {
            let y = f * 0.37;
            let x0 = (i % 5) as f64 * 0.9;
            n.segments.push(Segment::wire(layer, x0, y, x0 + 6.0, y));
            if i % 4 == 0 {
                n.segments.push(Segment::wire(layer, x0 + 6.0, y, x0 + 9.0, y));
            }
        } else {
            let x = f * 0.41;
            let y0 = (i % 7) as f64 * 0.8;
            n.segments.push(Segment::wire(layer, x, y0, x, y0 + 7.0));
        }
        nets.push(n);
    }
    let base = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    assert!(base.len() > 20, "the fixture must actually couple: {}", base.len());

    let mirror = |sign_x: f64, sign_y: f64| -> Vec<DefNet> {
        nets.iter()
            .map(|n| DefNet {
                name: n.name.clone(),
                pins: n.pins.clone(),
                segments: n
                    .segments
                    .iter()
                    .map(|s| Segment {
                        layer: s.layer.clone(),
                        x0: s.x0 * sign_x,
                        y0: s.y0 * sign_y,
                        x1: s.x1 * sign_x,
                        y1: s.y1 * sign_y,
                        width_um: s.width_um,
                    })
                    .collect(),
                vias: n.vias,
                via_points: n.via_points.clone(),
            })
            .collect()
    };
    for (sx, sy, what) in [
        (1.0, -1.0, "flipped top to bottom"),
        (-1.0, 1.0, "flipped left to right"),
        (-1.0, -1.0, "rotated 180"),
    ] {
        let got = extract_coupling(&mirror(sx, sy), &rules, &no_widths(), &no_widths());
        assert_eq!(base.len(), got.len(), "{what}: pair count");
        for (b, g) in base.iter().zip(&got) {
            assert_eq!((b.a.as_str(), b.b.as_str()), (g.a.as_str(), g.b.as_str()), "{what}: pairs");
            assert_eq!(b.cap_ff.to_bits(), g.cap_ff.to_bits(), "{what}: {}-{}", b.a, b.b);
        }
    }
}

/// Occlusion must never *create* coupling: whatever the visibility rule decides, a net pair's
/// cap can only be less than or equal to what the same pair reports with nothing in between.
/// A shadowing bug that mis-assigns run length would show up here even when the totals still fit.
#[test]
fn shadowing_only_ever_removes() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 3.0\n").unwrap();
    let nets: Vec<DefNet> = (0..25u32)
        .map(|i| {
            let x0 = (i % 4) as f64 * 1.3;
            hnet(&format!("n{i}"), x0, x0 + 8.0, i as f64 * 0.29)
        })
        .collect();
    let got = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    for c in &got {
        let i = nets.iter().position(|n| n.name == c.a).unwrap();
        let j = nets.iter().position(|n| n.name == c.b).unwrap();
        let alone = extract_coupling(&[nets[i].clone(), nets[j].clone()], &rules, &no_widths(), &no_widths());
        let bound = alone.first().map(|x| x.cap_ff).unwrap_or(0.0);
        assert!(
            c.cap_ff <= bound + 1e-12,
            "{}/{}: {} in context exceeds {} alone",
            c.a,
            c.b,
            c.cap_ff,
            bound
        );
    }
}

/// A power rail is metal that blocks and never couples. The reference ends its outward
/// walk on `isPower()` and books rail-adjacent field as GROUND, which is why its SPEF
/// carries no power nets at all — so a rail must remove the pair behind it and add none
/// of its own. On sky130 rows this is not a corner case: a met1 rail runs through every
/// standard-cell row.
#[test]
fn power_metal_blocks_and_is_never_reported() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 2.0\n").unwrap();
    let nets = [hnet("a", 0.0, 10.0, 0.0), hnet("c", 0.0, 10.0, 1.0)];
    let rail = vec![Segment {
        layer: "met1".into(),
        x0: 0.0,
        y0: 0.5,
        x1: 10.0,
        y1: 0.5,
        width_um: 0.2,
    }];

    // Without the grid the two signals couple straight through where the rail sits.
    let open = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    assert_eq!(open.len(), 1, "{open:?}");

    let blocked = extract_coupling_blocked(&nets, &rules, &no_widths(), &no_widths(), &rail);
    assert!(blocked.is_empty(), "coupled through a power rail: {blocked:?}");
}

/// A same-net wire is metal in the way. It blocks like any other wire, but a net does not
/// couple to itself, so the blocked pair simply disappears.
#[test]
fn same_net_metal_blocks_without_being_reported() {
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 2.0\n").unwrap();
    let mut a = hnet("a", 0.0, 10.0, 0.0);
    a.segments.push(Segment::wire("met1", 0.0, 0.5, 10.0, 0.5)); // a's own second wire
    let cc = extract_coupling(&[a, hnet("c", 0.0, 10.0, 1.0)], &rules, &no_widths(), &no_widths());
    // Only the a-wire at y=0.5 sees c; the one at y=0 is behind it.
    assert_eq!(cc.len(), 1, "{cc:?}");
    assert!((cc[0].cap_ff - 1.0).abs() < 1e-9, "cc={}", cc[0].cap_ff);
}

#[test]
fn parallel_result_is_bit_identical_to_serial() {
    // Band-partitioned parallel aggregation must reproduce the serial sweep exactly —
    // same pairs, same order, and the *same bits* for each coupling cap (contributions
    // summed in the same ascending-id order). A dense mesh with many multi-segment nets
    // so most pairs receive several contributions (where float add-order would bite).
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 2.0\n").unwrap();
    let mut nets: Vec<DefNet> = Vec::new();
    for i in 0..400u32 {
        let y = i as f64 * 0.3;
        // two collinear segments per net -> a neighbour couples to both (multi-contribution)
        nets.push(DefNet {
            name: format!("w{i}"),
            pins: vec![],
            segments: vec![
                Segment::wire("met1", 0.0, y, 5.0, y),
                Segment::wire("met1", 5.0, y, 11.0, y),
            ],
            vias: 0,
            via_points: Vec::new(),
        });
    }
    let run = |threads: usize| -> Vec<vyges_extract::coupling::CouplingCap> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| extract_coupling(&nets, &rules, &no_widths(), &no_widths()))
    };
    let serial = run(1);
    for t in [2usize, 8, 16] {
        let par = run(t);
        assert_eq!(serial.len(), par.len(), "{t} threads: pair count");
        for (s, p) in serial.iter().zip(&par) {
            assert_eq!(
                (s.a.as_str(), s.b.as_str()),
                (p.a.as_str(), p.b.as_str()),
                "{t}t order"
            );
            // bit-identical, not just approximately equal
            assert_eq!(
                s.cap_ff.to_bits(),
                p.cap_ff.to_bits(),
                "{t}t cap bits {}-{}",
                s.a,
                s.b
            );
        }
    }
}

#[test]
fn pair_cap_bounds_distinct_pairs() {
    // 4000 stacked wires -> 3999 adjacent coupling pairs. A tiny cap must bound the
    // number of distinct pairs held (never OOM on pathological density) while still
    // returning a valid, non-panicking result — the safety valve, not a crash.
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 0.7\n").unwrap();
    let n = 4000u32;
    let nets: Vec<DefNet> = (0..n)
        .map(|i| hnet(&format!("w{i}"), 0.0, 10.0, i as f64 * 0.5))
        .collect();
    let capped = extract_coupling_capped(&nets, &rules, &no_widths(), &no_widths(), 10);
    assert!(
        capped.len() <= 10,
        "distinct pairs bounded by the cap, got {}",
        capped.len()
    );
    // Every returned pair is a real one (present in the uncapped extraction) — the cap
    // drops pairs, it never invents or corrupts them.
    let full = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    for c in &capped {
        let m = full
            .iter()
            .find(|f| f.a == c.a && f.b == c.b)
            .expect("capped pair is real");
        assert!(
            (m.cap_ff - c.cap_ff).abs() < 1e-12,
            "capped pair keeps its full cap value"
        );
    }
}

#[test]
fn generous_cap_matches_uncapped() {
    // A cap above the pair count is a no-op: identical result to the uncapped path.
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 0.7\n").unwrap();
    let nets: Vec<DefNet> = (0..500u32)
        .map(|i| hnet(&format!("w{i}"), 0.0, 10.0, i as f64 * 0.5))
        .collect();
    let a = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    let b = extract_coupling_capped(&nets, &rules, &no_widths(), &no_widths(), usize::MAX);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(
            (x.a.as_str(), x.b.as_str()),
            (y.a.as_str(), y.b.as_str()),
            "same order"
        );
        assert!((x.cap_ff - y.cap_ff).abs() < 1e-12);
    }
}

#[test]
fn scales_to_thousands_of_nets() {
    // 4000 stacked parallel wires; only adjacent ones are within the cutoff, so
    // the answer is exactly 3999 coupling pairs. The naive all-pairs sweep would
    // be ~8M net-pair tests; the grid keeps it linear and finishes instantly.
    let rules = RcRules::parse("met1 0.1 0.05 0.1 0.5\ncouple_cutoff 0.7\n").unwrap();
    let n = 4000u32;
    let nets: Vec<DefNet> = (0..n)
        .map(|i| hnet(&format!("w{i}"), 0.0, 10.0, i as f64 * 0.5))
        .collect();
    let cc = extract_coupling(&nets, &rules, &no_widths(), &no_widths());
    assert_eq!(
        cc.len() as u32,
        n - 1,
        "only adjacent wires (gap 0.5 < 0.7) couple"
    );
}
