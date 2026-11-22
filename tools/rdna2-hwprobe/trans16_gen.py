import math, os, struct, sys

OPS = ["rcp", "rsq", "sqrt", "log", "exp", "sin", "cos"]
M64 = (1 << 64) - 1


def log2_table():
    return [min(round(math.log2(1 + j / 1024) * (1 << 32)), (1 << 32) - 1) for j in range(1024)]


def exp2_table():
    return [min(round((2 ** (j / 1024) - 1) * (1 << 32)), (1 << 32) - 1) for j in range(1024)]


def sin_coeffs():
    c = []
    for k in range(7):
        n = 2 * k + 1
        c.append(round((-1) ** k * (2 * math.pi) ** n / math.factorial(n) * (1 << 32)))
    return c


LOG2_T, EXP2_T, SIN_C = log2_table(), exp2_table(), sin_coeffs()
LN2_Q32 = round(math.log(2) * (1 << 32))


def r16(s, mag, ex):
    if mag == 0:
        return s << 15
    msb = mag.bit_length() - 1
    e16 = msb + ex
    if e16 > 15:
        return (s << 15) | 0x7C00
    sh = msb - 10 if e16 >= -14 else -24 - ex
    if sh <= 0:
        q = mag << -sh
    elif sh >= 64:
        q = 0
    else:
        q = mag >> sh
        rem = mag & ((1 << sh) - 1)
        half = 1 << (sh - 1)
        if rem > half or (rem == half and (q & 1)):
            q += 1
    if e16 >= -14:
        return (s << 15) | (((e16 + 15) << 10) + q - 1024)
    return (s << 15) | q


def split(h):
    s, e, m = h >> 15, (h >> 10) & 31, h & 0x3FF
    if e == 0:
        k = 10 - (m.bit_length() - 1)
        return s, m << k, -24 - k
    return s, m | 0x400, e - 25


def fixed24(h):
    s, M, E = split(h)
    X = M << (E + 24) if E + 24 >= 0 else M >> -(E + 24)
    return -X if s else X


def sinpoly(v):
    w = (v * v) >> 24
    acc = SIN_C[6]
    for k in range(5, -1, -1):
        acc = SIN_C[k] + ((acc * w) >> 24)
    return (acc * v) >> 24


def mirror(op, h):
    s, e, m = h >> 15, (h >> 10) & 31, h & 0x3FF
    nan = e == 31 and m != 0
    inf = e == 31 and m == 0
    zero = e == 0 and m == 0
    if nan:
        return h
    if op == "rcp":
        if zero:
            return (s << 15) | 0x7C00
        if inf:
            return s << 15
        _, M, E = split(h)
        q, r = (1 << 24) // M, (1 << 24) % M
        return r16(s, (q << 1) | (r != 0), -24 - E - 1)
    if op == "sqrt":
        if zero:
            return h
        if s:
            return 0xFE00
        if inf:
            return 0x7C00
        _, M, E = split(h)
        if E & 1:
            M, E = M << 1, E - 1
        N = M << 16
        S = math.isqrt(N)
        return r16(0, (S << 1) | (S * S != N), (E - 16) // 2 - 1)
    if op == "rsq":
        if zero:
            return (s << 15) | 0x7C00
        if s:
            return 0xFE00
        if inf:
            return 0
        _, M, E = split(h)
        if E & 1:
            M, E = M << 1, E - 1
        A, rr = (1 << 40) // M, (1 << 40) % M
        S = math.isqrt(A)
        return r16(0, (S << 1) | (S * S != A or rr != 0), -(E // 2) - 20 - 1)
    if op == "log":
        if zero:
            return 0xFC00
        if s:
            return 0xFE00
        if inf:
            return 0x7C00
        _, M, E = split(h)
        v = ((E + 10) << 32) + LOG2_T[M - 1024]
        return r16(1, -v, -32) if v < 0 else r16(0, v, -32)
    if op == "exp":
        if inf:
            return 0 if s else 0x7C00
        X = fixed24(h)
        X = max(min(X, 16 << 24), -(26 << 24))
        n = X >> 24
        F = X - (n << 24)
        j, lo = F >> 14, F & 0x3FFF
        P = (1 << 32) + EXP2_T[j]
        c = (lo * LN2_Q32) >> 24
        c2 = (c * c) >> 33
        mag = P + ((P * (c + c2)) >> 32)
        return r16(0, mag, n - 32)
    if inf:
        return 0xFE00
    if zero and op == "sin":
        return h
    r = fixed24(h) & 0xFFFFFF
    if op == "cos":
        r = (r + (1 << 22)) & 0xFFFFFF
    q, u = r >> 22, r & ((1 << 22) - 1)
    v = (1 << 22) - u if q & 1 else u
    if v == 0:
        return 0
    return r16(1 if q >= 2 else 0, sinpoly(v), -32)


def fnv(words):
    h = 0xCBF29CE484222325
    for w in words:
        for k in range(4):
            h = ((h ^ ((w >> (8 * k)) & 0xFF)) * 0x100000001B3) & M64
    return h


def main():
    d, tables_out, hw_out, gfx, machine = sys.argv[1:6]
    exc = []
    digests = []
    for i, op in enumerate(OPS):
        hw = struct.unpack("<65536I", open(os.path.join(d, f"v_{op}_f16_d3.bin"), "rb").read())
        hw = [x & 0xFFFF for x in hw]
        digests.append((op, fnv(hw)))
        bad = [(h, hw[h]) for h in range(65536) if mirror(op, h) != hw[h]]
        print(f"{op}: {len(bad)} exceptions", file=sys.stderr)
        exc += [(i, h, y) for h, y in bad]
    def rows(name, ty, items, per):
        out = [f"pub const {name}: [{ty}; {len(items)}] = ["]
        for k in range(0, len(items), per):
            out.append("    " + " ".join(x + "," for x in items[k:k + per]))
        out.append("];")
        return out
    t = []
    t += rows("LOG2_T", "u32", [f"{x:#010x}" for x in LOG2_T], 8)
    t += rows("EXP2_T", "u32", [f"{x:#010x}" for x in EXP2_T], 8)
    t += rows("SIN_C", "i64", [str(x) for x in SIN_C], 7)
    t.append(f"pub const LN2_Q32: u64 = {LN2_Q32:#x};")
    t += rows("EXCEPTIONS", "(u8, u16, u16)", [f"({o}, {h:#06x}, {y:#06x})" for o, h, y in exc], 4)
    open(tables_out, "w").write("\n".join(t) + "\n")
    w = [f'pub const SOURCE: &str = "{gfx}, {machine}";', f"pub const DIGESTS: [(&str, u64); {len(digests)}] = ["]
    w += [f'    ("v_{op}_f16", {h:#018x}),' for op, h in digests]
    w.append("];")
    open(hw_out, "w").write("\n".join(w) + "\n")


if __name__ == "__main__":
    main()
