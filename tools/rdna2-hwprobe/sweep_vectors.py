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
    o.append(f"pub const SOURCE: &str = \"{gfx}, {machine}\";")
    o.append("")

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
    o.append(f"pub const BFE: [(bool, u32, u64, u32, u64, u32); {len(bfe)}] = [")
    for s, bits, a, s1, got, scc in bfe:
        o.append(f"    ({str(s).lower()}, {bits}, {a:#x}, {s1:#010x}, {got:#x}, {scc}),")
    o.append("];")
    o.append("")

    x16, r16 = ins(d, "norm16.in"), rows(d, "norm16")
    for i, t in enumerate(x16):
        assert t[0] & 0xffff == i and t[1] & 0xffff == (i * 0x3d17) & 0xffff, "norm16.in is not the valu_probe input"
    for t in r16:
        assert t[2] >> 16 == vp.SENT >> 16 and t[3] >> 16 == vp.SENT >> 16, "v_cvt_norm_* upper half not preserved"
    cols = [("v_cvt_pknorm_i16_f16", 0, 0xffffffff), ("v_cvt_pknorm_u16_f16", 1, 0xffffffff),
            ("v_cvt_norm_i16_f16", 2, 0xffff), ("v_cvt_norm_u16_f16", 3, 0xffff)]
    o.append("pub const NORM16_DIGEST: [(&str, u64); 4] = [")
    for m, c, msk in cols:
        o.append(f"    (\"{m}\", {fnv([t[c] & msk for t in r16]):#018x}),")
    o.append("];")
    o.append(f"pub const NORM16: [(u32, u32, u32, u32, u32); {len(F16_PICK)}] = [")
    for i in F16_PICK:
        t = r16[i]
        o.append(f"    ({i:#06x}, {t[0]:#010x}, {t[1]:#010x}, {t[2] & 0xffff:#06x}, {t[3] & 0xffff:#06x}),")
    o.append("];")
    o.append("")

    x32, r32 = ins(d, "norm32.in"), rows(d, "norm32")
    n = len(vp.NRM_F32)
    for i in range(n * n):
        assert (x32[i][0], x32[i][1]) == (vp.NRM_F32[i % n], vp.NRM_F32[i // n]), "norm32.in grid differs"
    o.append("pub const NORM32_GRID_DIGEST: [(&str, u64); 2] = [")
    o.append(f"    (\"v_cvt_pknorm_i16_f32\", {fnv([r32[i][0] for i in range(n * n)]):#018x}),")
    o.append(f"    (\"v_cvt_pknorm_u16_f32\", {fnv([r32[i][1] for i in range(n * n)]):#018x}),")
    o.append("];")
    o.append(f"pub const NORM32: [(u32, u32, u32, u32); {len(F32_RANDOM)}] = [")
    for i in F32_RANDOM:
        o.append(f"    ({x32[i][0]:#010x}, {x32[i][1]:#010x}, {r32[i][0]:#010x}, {r32[i][1]:#010x}),")
    o.append("];")
    print("\n".join(o))


if __name__ == "__main__":
    main()
