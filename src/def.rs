//! Minimal DEF reader — the routed-geometry subset the extractor needs.
//!
//! Real DEF is large; v0 reads exactly what feeds RC extraction: the database
//! scale (`UNITS DISTANCE MICRONS`) and the `NETS` section — per net, its pin
//! connections and its `+ ROUTED` / `NEW` wire runs. For each run we accumulate
//! Manhattan segment lengths per layer and count via cuts. Routing width comes
//! from the rules deck (nominal per-layer width), so explicit width/`TAPER`/
//! `RECT`/`MASK`/`STYLE` decorations are skipped, and `( * y )` / `( x * )`
//! coordinate shorthand is resolved against the previous point.
//!
//! Pure std — fully unit-tested offline. SPECIAL NETS (PG) are out of scope for
//! signal extraction; only the `NETS` section is read.

#[derive(Debug, Clone)]
pub struct Segment {
    pub layer: String,
    pub len_um: f64,
}

#[derive(Debug, Clone)]
pub struct DefNet {
    pub name: String,
    pub pins: Vec<(String, String)>, // (instance, pin)
    pub segments: Vec<Segment>,
    pub vias: usize,
}

#[derive(Debug, Clone)]
pub struct Def {
    pub units_per_um: f64,
    pub nets: Vec<DefNet>,
}

#[derive(Debug)]
pub struct DefError(pub String);

impl std::fmt::Display for DefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "def error: {}", self.0)
    }
}
impl std::error::Error for DefError {}

/// Tokenize DEF, treating `(`, `)`, and `;` as standalone tokens.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for ch in text.chars() {
        match ch {
            '(' | ')' | ';' => {
                flush(&mut cur, &mut out);
                out.push(ch.to_string());
            }
            c if c.is_whitespace() => flush(&mut cur, &mut out),
            c => cur.push(c),
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Routing decorations that carry no wire and are not vias.
fn is_decoration(tok: &str) -> bool {
    matches!(
        tok,
        "TAPER" | "TAPERRULE" | "RECT" | "MASK" | "STYLE" | "VIRTUAL" | "ORIENT"
    )
}

/// Resolve one coordinate component: `*` reuses the previous value.
fn coord(tok: &str, prev: f64, scale: f64) -> Result<f64, DefError> {
    if tok == "*" {
        Ok(prev)
    } else {
        tok.parse::<f64>()
            .map(|v| v / scale)
            .map_err(|_| DefError(format!("bad coordinate {tok:?}")))
    }
}

pub fn parse(text: &str) -> Result<Def, DefError> {
    let t = tokenize(text);
    let mut scale = 1000.0;
    // UNITS DISTANCE MICRONS <n>
    for w in t.windows(4) {
        if w[0] == "UNITS" && w[1] == "DISTANCE" && w[2] == "MICRONS" {
            if let Ok(n) = w[3].parse::<f64>() {
                scale = n;
            }
        }
    }

    let mut nets = Vec::new();
    let mut i = match t.iter().position(|x| x == "NETS") {
        Some(p) => p,
        None => return Ok(Def { units_per_um: scale, nets }), // no signal nets
    };
    // skip `NETS <count> ;`
    while i < t.len() && t[i] != ";" {
        i += 1;
    }
    i += 1;

    while i < t.len() {
        if t[i] == "END" {
            break;
        }
        if t[i] != "-" {
            i += 1;
            continue;
        }
        i += 1; // consume '-'
        let name = t.get(i).cloned().unwrap_or_default();
        i += 1;

        let mut net = DefNet { name, pins: Vec::new(), segments: Vec::new(), vias: 0 };
        let mut in_routing = false;
        let mut layer: Option<String> = None;
        let mut prev: Option<(f64, f64)> = None;

        while i < t.len() && t[i] != ";" {
            match t[i].as_str() {
                "+" => {
                    // status keyword (ROUTED/FIXED/COVER/...) then layer
                    let status = t.get(i + 1).map(String::as_str).unwrap_or("");
                    if matches!(status, "ROUTED" | "FIXED" | "COVER" | "NOSHIELD") {
                        in_routing = true;
                        layer = t.get(i + 2).cloned();
                        prev = None;
                        i += 3;
                    } else {
                        // other `+ attribute ...` — skip the keyword, let the
                        // loop walk its values (none of interest to extraction)
                        i += 1;
                    }
                }
                "NEW" => {
                    layer = t.get(i + 1).cloned();
                    prev = None;
                    i += 2;
                }
                "(" => {
                    // gather the parenthesized group
                    let mut j = i + 1;
                    let mut inner = Vec::new();
                    while j < t.len() && t[j] != ")" {
                        inner.push(t[j].clone());
                        j += 1;
                    }
                    if !in_routing {
                        // connection: ( instance pin )
                        if inner.len() >= 2 {
                            net.pins.push((inner[0].clone(), inner[1].clone()));
                        }
                    } else if inner.len() >= 2 {
                        let (px, py) = prev.unwrap_or((0.0, 0.0));
                        let x = coord(&inner[0], px, scale)?;
                        let y = coord(&inner[1], py, scale)?;
                        if let (Some(l), Some((ox, oy))) = (&layer, prev) {
                            let len = (x - ox).abs() + (y - oy).abs();
                            if len > 0.0 {
                                net.segments.push(Segment { layer: l.clone(), len_um: len });
                            }
                        }
                        prev = Some((x, y));
                    }
                    i = j + 1; // past ')'
                }
                tok if is_decoration(tok) => {
                    i += 1; // skip decoration keyword; its args are coords/ints we ignore
                }
                _ => {
                    // a bare token inside a wire run is a via instance
                    if in_routing {
                        net.vias += 1;
                    }
                    i += 1;
                }
            }
        }
        nets.push(net);
        i += 1; // past ';'
    }

    Ok(Def { units_per_um: scale, nets })
}

pub fn load(path: &str) -> Result<Def, DefError> {
    let text = std::fs::read_to_string(path).map_err(|e| DefError(format!("{path}: {e}")))?;
    parse(&text)
}
