#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Correlate vyges-extract against a sign-off OpenRCX SPEF on a routed sky130 block.
#
# `cargo test` compares the engine against ITSELF: fixtures assert numbers the engine produced,
# which cannot catch a coefficient that was never right or a term that was silently switched off.
# This compares against an artifact from OUTSIDE — the parasitics the design's own OpenLane run
# produced with the foundry-reference extractor — and that comparison is what has actually found
# defects here.
#
# `fft_ctrl_tlul` is deliberately the block HELD OUT of every deck fit (see
# correlation/ground-vs-coupling.md), so this measures generalisation rather than reciting the
# calibration set back.
#
#   DESIGN=<clone of vyges-edge-sensor-soc> PDK_TLEF=<sky130 tech LEF> ./correlate-sky130.sh
set -euo pipefail

DESIGN=${DESIGN:?set DESIGN to a checkout carrying def/ and spef/}
DEF=$DESIGN/def/fft_ctrl_tlul.def
REF=$DESIGN/spef/fft_ctrl_tlul.nom.spef
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DECK=$ROOT/pdk/sky130A/sky130A.vyges-extract.rules
WORK=${WORK:-$(mktemp -d)}

for f in "$DEF" "$REF" "$DECK"; do
  [ -r "$f" ] || { echo "::error::missing input: $f"; exit 1; }
done
: "${PDK_TLEF:?set PDK_TLEF to the sky130 technology LEF}"
[ -r "$PDK_TLEF" ] || { echo "::error::missing tech LEF: $PDK_TLEF"; exit 1; }

echo "design : $DEF"
echo "ref    : $REF"
echo "deck   : $DECK"
echo "tlef   : $PDK_TLEF"
echo

cargo build --release --quiet --manifest-path "$ROOT/Cargo.toml"
EXTRACT=$ROOT/target/release/vyges-extract

cat > "$WORK/job.ext" <<EOF
design: fft_ctrl_tlul
def: $DEF
rules: $DECK
lef: $PDK_TLEF
corner: typical
temp: 25
EOF

"$EXTRACT" run "$WORK/job.ext" --json -o "$WORK/ours.json" -q
python3 "$ROOT/correlation/decompose.py" --ref "$REF" --ours "$WORK/ours.json" | tee "$WORK/report.txt"

# One grep-able contract line rather than scraping the human table above it, whose columns are
# free to change without anyone thinking about this script.
line=$(grep '^RATIO ' "$WORK/report.txt") || { echo "::error::decompose printed no RATIO line"; exit 1; }
field() { echo "$line" | tr ' ' '\n' | awk -F= -v k="$1" '$1==k{print $2; exit}'; }

nets=$(field nets)
[ "${nets:-0}" -gt 10000 ] || { echo "::error::only ${nets:-0} nets matched — the comparison did not really run"; exit 1; }

# Bounds, not equalities. The reference is a frozen artifact and the deck is fitted, so the
# ratios should sit near 1.0; the window is wide enough that ordinary numerical drift is not a
# red X, and narrow enough that losing a term — a zeroed coefficient, shielding switched off,
# coupling reverting 3x — cannot pass. Tightening these is a deliberate act, not a cleanup.
LO=0.85
HI=1.20
fail=0
for k in ground coupling total ground_median coupling_median; do
  v=$(field "$k")
  ok=$(awk -v v="$v" -v lo="$LO" -v hi="$HI" 'BEGIN{print (v>=lo && v<=hi) ? "ok" : "OUT"}')
  printf '  %-18s %-8s %s\n' "$k" "$v" "$ok"
  [ "$ok" = ok ] || fail=1
done

echo
if [ "$fail" -ne 0 ]; then
  echo "::error::a correlation ratio left [$LO, $HI] — the deck or the model moved"
  exit 1
fi
echo "correlation holds on $nets nets, all ratios within [$LO, $HI]"
