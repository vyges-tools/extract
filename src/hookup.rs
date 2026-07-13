//! Std-cell pin/device hookup resolution, shared by both SPEF paths (the native
//! DEF extractor and the KLayout front end). Maps a placed `(instance, pin)` to a
//! **direction** (cell-LEF `MACRO`/`PIN` `DIRECTION`, else liberty) and, for load
//! pins, the liberty **input capacitance** — the inputs a real `*CONN` needs to
//! mark drivers (`O`) vs loads (`I`) and to add per-load Cin.

use std::collections::HashMap;

use vyges_loom::def::Def;
use vyges_loom::lef::{Lef, PinDir};
use vyges_loom::liberty::{Dir as LibDir, Lib};

fn lib_dir_to_pin(d: LibDir) -> PinDir {
    match d {
        LibDir::In => PinDir::Input,
        LibDir::Out => PinDir::Output,
        LibDir::Inout => PinDir::Inout,
        LibDir::Other => PinDir::Unknown,
    }
}

/// Resolves `(instance, pin) → (direction, load-cap fF)` from a DEF (placement →
/// cell) + optional cell LEF (directions) + optional liberty (directions fallback
/// + input caps).
pub struct PinResolver {
    inst_cell: HashMap<String, String>,
    lef: Option<Lef>,
    lib: Option<Lib>,
}

impl PinResolver {
    /// Build from a parsed DEF plus optional cell-LEF / liberty paths. A `None`
    /// path is simply absent; a bad path is an error (fail loud on a wrong file).
    pub fn new(def: &Def, cell_lef: Option<&str>, lib: Option<&str>) -> Result<PinResolver, String> {
        let lef = match cell_lef {
            Some(p) => Some(Lef::load(p).map_err(|e| format!("{p}: {e}"))?),
            None => None,
        };
        let lib = match lib {
            Some(p) => Some(Lib::load(p).map_err(|e| format!("{p}: {e}"))?),
            None => None,
        };
        Ok(PinResolver::from_loaded(def, lef, lib))
    }

    /// Build from already-parsed collateral (e.g. reusing a LEF/liberty the caller
    /// loaded, or in tests).
    pub fn from_loaded(def: &Def, lef: Option<Lef>, lib: Option<Lib>) -> PinResolver {
        let inst_cell = def.comps.iter().map(|c| (c.name.clone(), c.cell.clone())).collect();
        PinResolver { inst_cell, lef, lib }
    }

    /// True when hookup can add information (there are placed cells AND at least one
    /// direction/cap source). When false, callers should skip hookup and emit the
    /// bare `*CONN` (direction defaults to load).
    pub fn active(&self) -> bool {
        !self.inst_cell.is_empty() && (self.lef.is_some() || self.lib.is_some())
    }

    /// Direction + load capacitance (fF) for a pin. Direction: cell LEF first, then
    /// liberty; cap: liberty input cap for input/inout pins (0 otherwise).
    pub fn resolve(&self, inst: &str, pin: &str) -> (PinDir, f64) {
        let cell = self.inst_cell.get(inst).map(|s| s.as_str());
        let mut dir = cell
            .and_then(|c| self.lef.as_ref().map(|l| l.pin_dir(c, pin)))
            .unwrap_or(PinDir::Unknown);
        if dir == PinDir::Unknown {
            if let Some(d) = cell
                .and_then(|c| self.lib.as_ref().and_then(|lb| lb.cell(c)))
                .and_then(|cc| cc.pins.get(pin))
                .map(|p| lib_dir_to_pin(p.direction))
            {
                dir = d;
            }
        }
        let cap_ff = if matches!(dir, PinDir::Input | PinDir::Inout) {
            cell.and_then(|c| self.lib.as_ref().and_then(|lb| lb.cell(c)))
                .map(|cc| cc.input_cap(pin) * 1e15) // liberty Farads → fF
                .unwrap_or(0.0)
        } else {
            0.0
        };
        (dir, cap_ff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> PinResolver {
        let def = Def::parse(
            "VERSION 5.8 ;\nDESIGN t ;\nUNITS DISTANCE MICRONS 1000 ;\n\
             COMPONENTS 2 ;\n- u1 INV_X1 + PLACED ( 0 0 ) N ;\n- u2 INV_X1 + PLACED ( 1 0 ) N ;\nEND COMPONENTS\n\
             NETS 1 ;\n- n ( u1 Y ) ( u2 A ) ;\nEND NETS\nEND DESIGN\n",
        )
        .unwrap();
        // write temp lef/lib
        let d = std::env::temp_dir();
        let lefp = d.join("vyges_hookup_test.lef");
        let libp = d.join("vyges_hookup_test.lib");
        std::fs::write(&lefp, "MACRO INV_X1\n PIN A\n  DIRECTION INPUT ;\n END A\n PIN Y\n  DIRECTION OUTPUT ;\n END Y\nEND INV_X1\n").unwrap();
        std::fs::write(&libp, "library(t){\n capacitive_load_unit (1, ff);\n cell(INV_X1){\n  pin(A){direction:input; capacitance:1.5;}\n  pin(Y){direction:output;}\n }\n}\n").unwrap();
        PinResolver::new(&def, Some(lefp.to_str().unwrap()), Some(libp.to_str().unwrap())).unwrap()
    }

    #[test]
    fn resolves_direction_and_cap() {
        let r = resolver();
        assert!(r.active());
        assert_eq!(r.resolve("u1", "Y"), (PinDir::Output, 0.0));
        let (d, c) = r.resolve("u2", "A");
        assert_eq!(d, PinDir::Input);
        assert!((c - 1.5).abs() < 1e-6);
        // unknown instance → unknown, no cap
        assert_eq!(r.resolve("zz", "A"), (PinDir::Unknown, 0.0));
    }
}
