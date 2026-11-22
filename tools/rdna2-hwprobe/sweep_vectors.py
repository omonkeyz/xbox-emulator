"""Write crates/tombox-xshader/tests/isa_sweep/rdna2_hw.rs from a `valu_probe.py gen` + run directory.

The ISA sweep (tests/isa_sweep.rs) checks the translator against CPU reference models. For s_bfe_* and
v_cvt_norm / v_cvt_pknorm those models were held back because nobody had checked them against silicon.
This records what an RDNA2 GPU returned for the `sbfe` and `norm` probes of valu_probe.py, so the test
`sweep_references_match_rdna2` can check the sweep's own reference functions against it:

  s_bfe       all 152 cases, literal (S0, S1, result, SCC)
  f16 norm    the probe inputs are regenerated in Rust (a = i, b = i * 0x3d17), so the record keeps an
              FNV-1a-64 digest of all 65536 results per instruction, plus literal vectors for edge inputs
  f32 pknorm  digest of the 26 x 26 edge grid (the sweep's NRM_F32 constants), plus 512 literal vectors
              from the random part (Python's random inputs are not reproducible in Rust)

Usage: sweep_vectors.py <probe dir> <gfx name> <description of the machine> > rdna2_hw.rs
Needs valu_probe.py (same directory) for the s_bfe case list.
"""
import os, struct, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import valu_probe as vp

N = 65536
F16_EDGES = [0x0000, 0x0001, 0x03ff, 0x0400, 0x37ff, 0x3800, 0x3801, 0xb7ff, 0xb800, 0xb801, 0x3bff, 0x3c00, 0x3c01,
             0xbbff, 0xbc00, 0xbc01, 0x7bff, 0xfbff, 0x7c00, 0xfc00, 0x7c01, 0x7e00, 0xfe00, 0x8000, 0x8001, 0x83ff]
F16_PICK = sorted(set(F16_EDGES) | set(range(0, N, 2048)))
F32_RANDOM = [676 + 126 * k for k in range(512)]


def fnv(words):
    h = 0xcbf29ce484222325
    for w in words:
        for k in range(4):
            h = ((h ^ ((w >> (8 * k)) & 0xff)) * 0x100000001b3) & 0xffffffffffffffff
    return h


def rows(d, name):
    b = open(f"{d}/{name}.out", "rb").read()
    return [struct.unpack_from('<16I', b, 64 * i) for i in range(N)]


def ins(d, name):
    b = open(f"{d}/{name}", "rb").read()
    return [struct.unpack_from('<4I', b, 16 * i) for i in range(N)]


def main():
    d, gfx, machine = sys.argv[1], sys.argv[2], sys.argv[3]
    o = []
    o.append(f"// RDNA2 hardware results ({gfx}, {machine}) for the s_bfe and normalized-conversion")
    o.append("// probes of tools/rdna2-hwprobe/valu_probe.py, written by tools/rdna2-hwprobe/sweep_vectors.py.")
    o.append("// `sweep_references_match_rdna2` checks the sweep's reference models (ops_s::bfe64, ops_v::norm16)")
    o.append("// against them, so the translator (checked against those models by `isa_sweep`) is tied to silicon.")
    o.append("// Do not edit by hand.")
    o.append("")

    # ---- s_bfe ----
    bfe = []
    for name, _, (sg, bits, cases) in vp.sbfe_kernels():
        r = rows(d, name)
        assert all(x == r[0] for x in r), f"{name}: threads differ"
        for j, (off, w, junk) in enumerate(cases):
            if bits == 32:
                got, scc, a = r[0][2 * j], r[0][2 * j + 1], vp.A32
            else:
                got, scc, a = r[0][3 * j] | r[0][3 * j + 1] << 32, r[0][3 * j + 2], vp.A64
            assert scc in (0, 1)
            bfe.append((sg == "i", bits, a, vp.s1(off, w, junk), got, scc))
    o.append("/// (signed, bits, S0, S1, result, SCC) for s_bfe_{u,i}{32,64}.")
    o.append(f"pub const BFE: [(bool, u32, u64, u32, u64, u32); {len(bfe)}] = [")
    for s, bits, a, s1, got, scc in bfe:
        o.append(f"    ({str(s).lower()}, {bits}, {a:#x}, {s1:#010x}, {got:#x}, {scc}),")
    o.append("];")
    o.append("")

    # ---- f16 norm (inputs a = i, b = i * 0x3d17 & 0xffff) ----
    x16, r16 = ins(d, "norm16.in"), rows(d, "norm16")
    for i, t in enumerate(x16):
        assert t[0] & 0xffff == i and t[1] & 0xffff == (i * 0x3d17) & 0xffff, "norm16.in is not the valu_probe input"
    for t in r16:
        assert t[2] >> 16 == vp.SENT >> 16 and t[3] >> 16 == vp.SENT >> 16, "v_cvt_norm_* upper half not preserved"
    cols = [("v_cvt_pknorm_i16_f16", 0, 0xffffffff), ("v_cvt_pknorm_u16_f16", 1, 0xffffffff),
            ("v_cvt_norm_i16_f16", 2, 0xffff), ("v_cvt_norm_u16_f16", 3, 0xffff)]
    o.append("/// FNV-1a-64 over the results for every input i in 0..65536 (a = i, b = i * 0x3d17 as f16 bit")
    o.append("/// patterns; the v_cvt_norm_* digests cover the low 16 bits; the upper half was preserved on every lane).")
    o.append("pub const NORM16_DIGEST: [(&str, u64); 4] = [")
    for m, c, msk in cols:
        o.append(f"    (\"{m}\", {fnv([t[c] & msk for t in r16]):#018x}),")
    o.append("];")
    o.append("/// (i, pknorm_i16_f16, pknorm_u16_f16, norm_i16_f16 low half, norm_u16_f16 low half) for edge inputs.")
    o.append(f"pub const NORM16: [(u32, u32, u32, u32, u32); {len(F16_PICK)}] = [")
    for i in F16_PICK:
        t = r16[i]
        o.append(f"    ({i:#06x}, {t[0]:#010x}, {t[1]:#010x}, {t[2] & 0xffff:#06x}, {t[3] & 0xffff:#06x}),")
    o.append("];")
    o.append("")

    # ---- f32 pknorm ----
    x32, r32 = ins(d, "norm32.in"), rows(d, "norm32")
    n = len(vp.NRM_F32)
    for i in range(n * n):
        assert (x32[i][0], x32[i][1]) == (vp.NRM_F32[i % n], vp.NRM_F32[i // n]), "norm32.in grid differs"
    o.append(f"/// FNV-1a-64 over the results for the {n} x {n} grid a = NRM_F32[i % {n}], b = NRM_F32[i / {n}].")
    o.append("pub const NORM32_GRID_DIGEST: [(&str, u64); 2] = [")
    o.append(f"    (\"v_cvt_pknorm_i16_f32\", {fnv([r32[i][0] for i in range(n * n)]):#018x}),")
    o.append(f"    (\"v_cvt_pknorm_u16_f32\", {fnv([r32[i][1] for i in range(n * n)]):#018x}),")
    o.append("];")
    o.append("/// (a, b, pknorm_i16_f32, pknorm_u16_f32) for inputs drawn uniformly from [-1.25, 1.25].")
    o.append(f"pub const NORM32: [(u32, u32, u32, u32); {len(F32_RANDOM)}] = [")
    for i in F32_RANDOM:
        o.append(f"    ({x32[i][0]:#010x}, {x32[i][1]:#010x}, {r32[i][0]:#010x}, {r32[i][1]:#010x}),")
    o.append("];")
    print("\n".join(o))


if __name__ == "__main__":
    main()
