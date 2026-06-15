import subprocess, numpy as np, sys, os
# Reproduce on your own host: point these at a working dir with the routed DEF/LEF,
# the built vyges-extract, and an OpenRCX reference SPEF for the same block.
#   VYGES_PEX_DIR=<workdir> VYGES_EXTRACT=<bin> VYGES_OPENRCX_SPEF=<spef> python3 calibrate.py
os.chdir(os.environ.get("VYGES_PEX_DIR", "."))
EXT = os.environ.get("VYGES_EXTRACT", "vyges-extract")
GOLD = os.environ.get("VYGES_OPENRCX_SPEF", "counter.nom.spef")
LAYERS = ["li1","met1","met2","met3"]
# illustrative per-layer (res, cap, coupling, s_ref) — the basis we scale
BASE = {"li1":(12.8,0.060,0.030,0.17),"met1":(0.125,0.078,0.050,0.14),
        "met2":(0.125,0.072,0.044,0.14),"met3":(0.047,0.068,0.040,0.30)}

def deck(path, caps):  # caps: {layer: (cap,coup)} ; res kept, interlayer omitted
    with open(path,"w") as f:
        for L in LAYERS:
            r=BASE[L][0]; c,cp=caps.get(L,(0.0,0.0)); s=BASE[L][3]
            f.write(f"{L}\t{r}\t{c}\t{cp}\t{s}\n")
        f.write("via\t9.3\ncouple_cutoff 2.0\n")

def job(rules):
    with open("cal.ext","w") as f:
        f.write(f"design: counter\ndef: counter.def\nrules: {rules}\nlef: counter.lef\ncorner: typical\ntemp: 25\n")

def parse(path, to_ff):
    names={}; caps={}; innm=False
    for line in open(path):
        s=line.strip()
        if s.startswith("*NAME_MAP"): innm=True; continue
        if innm:
            t=s.split()
            if len(t)==2 and t[0][1:].isdigit(): names[t[0]]=t[1]; continue
            if s.startswith("*"): innm=False
        if s.startswith("*D_NET"):
            t=s.split(); caps[t[1]]=float(t[2])*(1000.0 if to_ff else 1.0)
    return {names.get(i,i):c for i,c in caps.items()}

def run(rules, out):
    job(rules)
    subprocess.run([EXT,"run","cal.ext","-o",out],check=True,capture_output=True)
    return parse(out, False)

gold = parse(GOLD, True)
# isolation runs: one layer active at a time -> design matrix
cols=[]
for L in LAYERS:
    deck(f"iso_{L}.rules", {L:(BASE[L][1],BASE[L][2])})
    cols.append(run(f"iso_{L}.rules", f"iso_{L}.spef"))
nets=sorted(set(gold) & set(cols[0]))
A=np.array([[cols[k].get(n,0.0) for k in range(len(LAYERS))] for n in nets])
b=np.array([gold[n] for n in nets])
# non-negative-ish least squares (clip negatives, refit)
alpha,_,_,_=np.linalg.lstsq(A,b,rcond=None)
alpha=np.clip(alpha,0,None)
print("per-layer scale alpha:", {LAYERS[i]:round(float(alpha[i]),3) for i in range(len(LAYERS))})
# calibrated deck
cal={L:(BASE[L][1]*alpha[i],BASE[L][2]*alpha[i]) for i,L in enumerate(LAYERS)}
deck("sky130A.vyges-extract.rules", cal)
vyg=run("sky130A.vyges-extract.rules","vyges_calfit.spef")
common=sorted(set(gold)&set(vyg))
allr=[vyg[n]/gold[n] for n in common if gold[n]>0]
tv,tg=sum(vyg[n] for n in common),sum(gold[n] for n in common)
import statistics as st
print(f"CALIBRATED: nets {len(common)}  mean {st.mean(allr):.3f}  median {st.median(allr):.3f}  min {min(allr):.3f}  max {max(allr):.3f}")
print(f"  total vyges {tv:.1f} fF  rcx {tg:.1f} fF  ratio {tv/tg:.3f}")
print(f"  stdev(ratio) {st.pstdev(allr):.3f}")

print("\n--- variant: li1 pinned physical (break collinearity), fit met1-3 to residual ---")
LI1_A = 1.0  # keep illustrative li1 cap (physical, small local-interconnect cap)
b2 = b - LI1_A * A[:,0]
A2 = A[:,1:]
a2,_,_,_ = np.linalg.lstsq(A2, b2, rcond=None); a2=np.clip(a2,0,None)
alpha2 = np.array([LI1_A]+list(a2))
print("alpha:", {LAYERS[i]:round(float(alpha2[i]),3) for i in range(4)})
cal2={L:(BASE[L][1]*alpha2[i],BASE[L][2]*alpha2[i]) for i,L in enumerate(LAYERS)}
deck("sky130A.vyges-extract.rules", cal2)
vyg=run("sky130A.vyges-extract.rules","vyges_calfit.spef")
common=sorted(set(gold)&set(vyg)); allr=[vyg[n]/gold[n] for n in common if gold[n]>0]
import statistics as st
tv,tg=sum(vyg[n] for n in common),sum(gold[n] for n in common)
print(f"CALIBRATED(li1-pinned): mean {st.mean(allr):.3f}  median {st.median(allr):.3f}  min {min(allr):.3f}  max {max(allr):.3f}  stdev {st.pstdev(allr):.3f}")
print(f"  total ratio {tv/tg:.3f}")
print("=== final deck ==="); print(open("sky130A.vyges-extract.rules").read())
