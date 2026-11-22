import os, struct, sys

M64 = (1 << 64) - 1


def fnv(words):
    h = 0xCBF29CE484222325
    for w in words:
        for k in range(4):
            h = ((h ^ ((w >> (8 * k)) & 0xFF)) * 0x100000001B3) & M64
    return h


def table(path, n):
    rows = {}
    for line in open(path):
        f = line.split()
        if len(f) != 9 or f[1] != "0":
            raise SystemExit(f"{path}: unsolved or malformed row: {line.strip()}")
        c0, c1, c2, q, m, _, dn = (int(x) for x in f[2:])
        rows[int(f[0])] = [c0, c1, c2, q, m, dn]
    if sorted(rows) != list(range(n)):
        raise SystemExit(f"{path}: expected {n} rows")
    return [rows[i] for i in range(n)]


def words(path, n):
    return list(struct.unpack(f"<{n}I", open(path, "rb").read(4 * n)))


def rust_rows(name, rows):
    out = [f"pub const {name}: [[i64; 6]; {len(rows)}] = ["]
    for r in rows:
        out.append("    [" + ", ".join(str(v) for v in r) + "],")
    out.append("];")
    return out


def main():
    d, tables_out, hw_out, gfx, machine = sys.argv[1:6]
    t = []
    t += rust_rows("RCP", table(os.path.join(d, "rcp64_tables.txt"), 32))
    t += rust_rows("RSQ0", table(os.path.join(d, "rsq64_e0.txt"), 32))
    t += rust_rows("RSQ1", table(os.path.join(d, "rsq64_e1.txt"), 32))
    t += rust_rows("SQRT0", table(os.path.join(d, "sqrt64_e0.txt"), 16))
    t += rust_rows("SQRT1", table(os.path.join(d, "sqrt64_e1.txt"), 16))
    open(tables_out, "w").write("\n".join(t) + "\n")
    ranges = [("v_rcp_f32", "v_rcp_f32_d3_e0", 0x3F800000), ("v_rcp_f32", "v_rcp_f32_d3_e1", 0x40000000),
              ("v_rcp_iflag_f32", "v_rcp_iflag_f32_d3_e0", 0x3F800000), ("v_rcp_iflag_f32", "v_rcp_iflag_f32_d3_e1", 0x40000000),
              ("v_rsq_f32", "v_rsq_f32_d3_e0", 0x3F800000), ("v_rsq_f32", "v_rsq_f32_d3_e1", 0x40000000),
              ("v_sqrt_f32", "v_sqrt_f32_d3_e0", 0x3F800000), ("v_sqrt_f32", "v_sqrt_f32_d3_e1", 0x40000000),
              ("v_log_f32", "v_log_f32_d3_e0", 0x3F800000), ("v_log_f32", "v_log_f32_d3_e1", 0x40000000),
              ("v_exp_f32", "v_exp_f32_d3_m1p2", 0x3F800000), ("v_exp_f32", "v_exp_f32_d3_m05", 0x3F000000), ("v_exp_f32", "v_exp_f32_d3_n05", 0xBF000000),
              ("v_sin_f32", "v_sin_f32_d3_e125", 0x3E000000), ("v_sin_f32", "v_sin_f32_d3_e25", 0x3E800000),
              ("v_cos_f32", "v_cos_f32_d3_e125", 0x3E000000), ("v_cos_f32", "v_cos_f32_d3_e25", 0x3E800000)]
    for base in (0x3F000000, 0x00800000, 0x7F000000):
        ranges.append(("v_log_f32", f"x_log_{base:08x}", base))
    for base in (0x40000000, 0xC0000000, 0x42800000, 0xC2800000, 0x33800000):
        ranges.append(("v_exp_f32", f"x_exp_{base:08x}", base))
    for op in ("sin", "cos"):
        for base in (0x3F000000, 0x3F800000, 0x40800000, 0x4A800000, 0x39800000, 0x39000000, 0xBE800000, 0x00800000):
            ranges.append((f"v_{op}_f32", f"x_{op}_{base:#010x}", base))
    w = [f'pub const SOURCE: &str = "{gfx}, {machine}";']
    w.append(f"pub const RANGE_DIGESTS: [(&str, u32, u64); {len(ranges)}] = [")
    for op, fn, base in ranges:
        ws = words(os.path.join(d, "data", fn + ".bin"), 1 << 23)
        w.append(f'    ("{op}", {base:#010x}, {fnv(ws):#018x}),')
    w.append("];")
    ops = ("rcp", "rcp_iflag", "rsq", "sqrt", "log", "exp", "sin", "cos")
    w.append(f"pub const EDGE_DIGESTS: [(&str, u32, u64); {2 * len(ops)}] = [")
    for op in ops:
        for m in (3, 0):
            ws = words(os.path.join(d, "data", f"edge_{op}_d{m}.bin"), 1 << 20)
            w.append(f'    ("v_{op}_f32", {m}, {fnv(ws):#018x}),')
    w.append("];")
    f64 = []
    for op in ("rcp", "rsq", "sqrt"):
        for m in (3, 0):
            for lo in ("0", "0x9e3779b9", "0xffffffff"):
                f64.append((op, 0, int(lo, 16) if lo != "0" else 0, 0, m, f"edge64_{op}_d{m}_{lo}", 1 << 20))
        for hb in (0x3FF00000, 0x40000000):
            f64.append((op, 1, hb, 0, 0, f"cz_{op}_{hb:#010x}", 1 << 23))
        for hb in (0x3FFFFFFF, 0x400FFFFF):
            f64.append((op, 2, hb, 0, 0, f"co_{op}_{hb:#010x}", 1 << 23))
        for hb in (0x3FF00000, 0x40000000):
            for ml in (0, 7):
                for eh in range(4):
                    lo = (ml << 29) | (eh << 27) | (15 << 23)
                    f64.append((op, 3, hb, lo, 0, f"exh/{op}_{hb:#010x}_{lo}", 1 << 20))
    w.append(f"pub const F64_DIGESTS: [(&str, u8, u32, u32, u8, u64); {len(f64)}] = [")
    for op, kind, a, b, m, fn, n in f64:
        ws = words(os.path.join(d, "data", fn + ".bin"), 2 * n)
        w.append(f'    ("v_{op}_f64", {kind}, {a:#010x}, {b:#010x}, {m}, {fnv(ws):#018x}),')
    w.append("];")
    open(hw_out, "w").write("\n".join(w) + "\n")


if __name__ == "__main__":
    main()
