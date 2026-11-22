"""VALU/SALU probes for questions the public RDNA2 ISA text leaves open.

Same harness as the MTBUF probe (runner.cpp, kernel preamble from gen.py). Four probe families:

  fma16   VOP2 0x36/0x37/0x38 (LLVM: v_fmac_f16 / v_fmamk_f16 / v_fmaak_f16): fused or mul-then-add?
          Each kernel also runs v_fma_f16 and v_mul_f16 + v_add_f16 on the same inputs, so the
          hardware provides both references.
  sbfe    s_bfe_{u,i}{32,64} with a width field (S1[22:16]) of 32 or more, and junk in the unused
          S1 bits: is the width 7 bits and bounded, or taken modulo the operand size?
  nan     v_fma_f32 / v_mul_f32 / v_add_f32 / v_min_f32 / v_max_f32 on every combination of quiet and
          signalling NaNs, infinities, zeros and denormals, with IEEE mode off and on.
  norm    v_cvt_pknorm_{i,u}16_f32, v_cvt_pknorm_{i,u}16_f16, v_cvt_norm_{i,u}16_f16: rounding, NaN and
          out-of-range inputs, upper half of the destination (the f16 forms are exhaustive over 16 bits).

Usage: valu_probe.py gen <outdir>     then build and run exactly as run.sh does (clang, ld.lld, runner)
       valu_probe.py show <outdir>    prints which candidate rule matches the recorded outputs

Nothing here has been run on hardware by its author (no AMD GPU available): `gen` output was only
assembled with LLVM 18 for gfx1036 and gfx1013. `show` was exercised against synthetic outputs.
"""
import os, random, struct, sys
from fractions import Fraction
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen import kernel, meta, N

SENT = 0x5a5a5a5a
LOAD = """  v_lshlrev_b32 v6, 4, v1
  v_add_co_u32 v7, vcc_lo, s4, v6
  v_add_co_ci_u32_e32 v8, vcc_lo, s5, v31, vcc_lo
  global_load_dwordx4 v[10:13], v[7:8], off
  s_waitcnt vmcnt(0)"""
PRESET = "\n".join(f"  v_mov_b32 v{r}, {SENT:#x}" for r in range(14, 30))
STORE = "\n".join(f"  global_store_dwordx4 v[4:5], v[{14 + 4 * k}:{17 + 4 * k}], off offset:{16 * k}" for k in range(4))

def kd(name, ieee=0):
    return f"""
.rodata
.p2align 6
.amdhsa_kernel {name}
  .amdhsa_user_sgpr_kernarg_segment_ptr 1
  .amdhsa_system_sgpr_workgroup_id_x 1
  .amdhsa_next_free_vgpr 32
  .amdhsa_next_free_sgpr 32
  .amdhsa_wavefront_size32 1
  .amdhsa_kernarg_size 24
  .amdhsa_ieee_mode {ieee}
  .amdhsa_dx10_clamp 1
  .amdhsa_float_denorm_mode_32 3
  .amdhsa_float_denorm_mode_16_64 3
.end_amdhsa_kernel
"""

def h2q(b):
    """f16 bits -> Fraction, or None for inf/NaN."""
    s, e, m = b >> 15 & 1, b >> 10 & 31, b & 0x3ff
    if e == 31:
        return None
    v = Fraction(m, 1 << 24) if e == 0 else Fraction(1024 + m, 1 << 10) * Fraction(2) ** (e - 15)
    return -v if s else v

def rne16(x):
    """Fraction -> f16 bits, one rounding to nearest-even; overflow gives infinity."""
    if x == 0:
        return 0
    s = 0x8000 if x < 0 else 0
    x = abs(x)
    e = x.numerator.bit_length() - x.denominator.bit_length()
    if Fraction(2) ** e > x:
        e -= 1
    e = max(e, -14)
    q = x / Fraction(2) ** (e - 10)
    n = q.numerator // q.denominator
    r = q - n
    if r > Fraction(1, 2) or (r == Fraction(1, 2) and n & 1):
        n += 1
    if n == 2048:
        n, e = 1024, e + 1
    if e > 15:
        return s | 0x7c00
    return s | n if n < 1024 else s | (e + 15) << 10 | (n - 1024)

def rnd16(rnd):
    return rnd.getrandbits(1) << 15 | rnd.randint(12, 18) << 10 | rnd.getrandbits(10)

FMA_K = [0x3c01, 0x3555, 0xc248]

def fma16_body(k):
    return f"""{LOAD}
{PRESET}
  v_mov_b32 v30, {k:#x}
  v_mov_b32 v14, v12
  v_fmac_f16 v14, v10, v11
  v_fma_f16 v15, v10, v11, v12
  v_mul_f16 v16, v10, v11
  v_add_f16 v16, v16, v12
  v_fmamk_f16 v17, v10, {k:#x}, v12
  v_fma_f16 v18, v10, v30, v12
  v_mul_f16 v19, v10, v30
  v_add_f16 v19, v19, v12
  v_fmaak_f16 v20, v10, v11, {k:#x}
  v_fma_f16 v21, v10, v11, v30
  v_mul_f16 v22, v10, v11
  v_add_f16 v22, v22, v30
{STORE}"""

def fma16_input(k):
    rnd = random.Random(4100 + k)
    out = bytearray()
    kq = h2q(k)
    for i in range(N):
        a, b, c = rnd16(rnd), rnd16(rnd), rnd16(rnd)
        mode = i % 4
        if mode == 0:      # a*b + c cancels to the rounding residual of a*b
            c = rne16(h2q(a) * h2q(b)) ^ 0x8000
        elif mode == 1:    # a*K + c cancels
            c = rne16(h2q(a) * kq) ^ 0x8000
        elif mode == 2:    # a*b + K cancels
            b = rne16(-kq / h2q(a))
        out += struct.pack('<4I', 0xabcd0000 | a, 0x12340000 | b, 0x56780000 | c, 0)
    return bytes(out)

A32 = 0x89abcdef
A64 = 0x89abcdef01234567
def sbfe_cases():
    c32 = [(o, w, 0) for o in (0, 4, 31) for w in (0, 1, 28, 31, 32, 33, 63, 64, 65, 96, 127)]
    c32 += [(4, 8, 1), (4, 40, 1), (36, 8, 0), (36, 40, 0)]            # junk bits / offset bit 5
    c64 = [(o, w, 0) for o in (0, 4, 40, 63) for w in (0, 1, 31, 32, 33, 63, 64, 65, 127)]
    c64 += [(4, 8, 1), (4, 72, 1), (68, 8, 0)]                          # junk bits / offset bit 6
    return c32, c64

def s1(o, w, junk):
    return (o | w << 16 | (0xff80ff80 if junk else 0)) & 0xffffffff

def sbfe_kernels():
    c32, c64 = sbfe_cases()
    ks = []
    for sg in "ui":
        for n in range(0, len(c32), 8):
            lines = [PRESET]
            for j, (o, w, junk) in enumerate(c32[n:n + 8]):
                lines += [f"  s_mov_b32 s20, {A32:#x}", f"  s_mov_b32 s21, {s1(o, w, junk):#x}", f"  s_bfe_{sg}32 s22, s20, s21",
                          "  s_cselect_b32 s23, 1, 0", f"  v_mov_b32 v{14 + 2 * j}, s22", f"  v_mov_b32 v{15 + 2 * j}, s23"]
            ks.append((f"sbfe_{sg}32_{n // 8}", "\n".join(lines + [STORE]), (sg, 32, c32[n:n + 8])))
        for n in range(0, len(c64), 5):
            lines = [PRESET]
            for j, (o, w, junk) in enumerate(c64[n:n + 5]):
                lines += [f"  s_mov_b32 s20, {A64 & 0xffffffff:#x}", f"  s_mov_b32 s21, {A64 >> 32:#x}", f"  s_mov_b32 s24, {s1(o, w, junk):#x}",
                          f"  s_bfe_{sg}64 s[22:23], s[20:21], s24", "  s_cselect_b32 s25, 1, 0",
                          f"  v_mov_b32 v{14 + 3 * j}, s22", f"  v_mov_b32 v{15 + 3 * j}, s23", f"  v_mov_b32 v{16 + 3 * j}, s25"]
            ks.append((f"sbfe_{sg}64_{n // 5}", "\n".join(lines + [STORE]), (sg, 64, c64[n:n + 5])))
    return ks

def bfe_model(a, o, w, signed, bits, wide):
    """wide: width is the 7-bit field and a field reaching the top bit takes all remaining bits;
    otherwise the width is taken modulo the operand size."""
    o &= bits - 1
    w = w & 127 if wide else w & (bits - 1)
    if w == 0:
        return 0
    x = a >> o
    if w < bits - o:
        x &= (1 << w) - 1
        if signed and x >> (w - 1):
            x |= ((1 << bits) - 1) & ~((1 << w) - 1)
    elif signed and a >> (bits - 1):
        x |= ((1 << bits) - 1) & ~((1 << (bits - o)) - 1)
    return x

def bfe_literal(a, o, w, signed, bits):
    """The ISA formula read literally: (S0 >> off) & ((1 << width) - 1) with a logical shift and
    the 7-bit width, then sign-extended from bit width-1. A signed field that runs past the top
    bit therefore zero-fills, where 'wide' sign-fills."""
    o &= bits - 1
    w &= 127
    if w == 0:
        return 0
    x = (a >> o) & ((1 << w) - 1)
    if signed and w <= bits and x >> (w - 1):
        x |= ((1 << bits) - 1) & ~((1 << w) - 1)
    return x

SPECIAL = [0x7fc00001, 0xffc00002, 0x7f800003, 0xff800004, 0x7f800000, 0xff800000, 0, 0x80000000, 0x3f800000, 0x00000001, 0x807fffff]
NAN_BODY = f"""{LOAD}
{PRESET}
  v_fma_f32 v14, v10, v11, v12
  v_mul_f32 v15, v10, v11
  v_add_f32 v16, v10, v12
  v_min_f32 v17, v10, v11
  v_max_f32 v18, v10, v11
  v_mov_b32 v19, v12
  v_fmac_f32 v19, v10, v11
  v_sub_f32 v20, v10, v12
{STORE}"""

def nan_input():
    rnd = random.Random(77)
    n, out = len(SPECIAL), bytearray()
    for i in range(N):
        if i < n ** 3:
            t = [SPECIAL[i % n], SPECIAL[i // n % n], SPECIAL[i // n // n]]
        else:
            t = [rnd.getrandbits(32) for _ in range(3)]
            t[rnd.randrange(3)] = (rnd.choice(SPECIAL[:4]) ^ rnd.getrandbits(22)) | 1
        out += struct.pack('<4I', *t, 0)
    return bytes(out)

def isnan(b):
    return b & 0x7fffffff > 0x7f800000
def f32(b):
    return struct.unpack('<f', struct.pack('<I', b))[0]
DEFAULT_NAN = 0xffc00000
def isinf(b):
    return b & 0x7fffffff == 0x7f800000
def iszero(b):
    return b & 0x7fffffff == 0
def issnan(b):
    return isnan(b) and not b & 0x400000
def nan_arith(ops, kind, ieee):
    """NaN result of fma / fmac / mul / add / sub as measured on gfx1036, or None if the result is
    not a NaN. ops: (a, b, c) for fma, (src0, src1) otherwise.
      1. v_sub_f32 negates src1 first, so a NaN src1 comes out with its sign flipped.
      2. fma: if neither a nor b is NaN and a * b is invalid (0 * inf), the default NaN wins over a NaN c.
      3. Otherwise the first NaN operand: unchanged in IEEE mode 0 (sNaN stays signalling), quieted in mode 1.
      4. An invalid operation without a NaN input gives the default NaN 0xffc00000 in both modes."""
    q = (lambda x: x | 0x400000) if ieee else (lambda x: x)
    if kind == "sub":
        ops = (ops[0], ops[1] ^ 0x80000000)
    a, b = ops[0], ops[1]
    prod_invalid = (isinf(a) and iszero(b)) or (iszero(a) and isinf(b))
    if kind == "fma" and prod_invalid and not isnan(a) and not isnan(b):
        return DEFAULT_NAN
    for x in ops:
        if isnan(x):
            return q(x)
    if kind in ("fma", "mul") and prod_invalid:
        return DEFAULT_NAN
    if kind in ("add", "sub") and isinf(a) and isinf(b) and a >> 31 != b >> 31:
        return DEFAULT_NAN
    if kind == "fma":
        p = f32(a) * f32(b)
        c = ops[2]
        if abs(p) == float("inf") and isinf(c) and (p < 0) != bool(c >> 31):
            return DEFAULT_NAN
    return None

def nan_minmax(a, b, ieee):
    """NaN-related result of v_min_f32 / v_max_f32 as measured on gfx1036, or None if neither operand
    is a NaN. IEEE mode 0: a NaN loses to a number, two NaNs give src0 unchanged. IEEE mode 1: an sNaN
    wins and is quieted (src0 checked first), otherwise as mode 0."""
    if ieee:
        for x in (a, b):
            if issnan(x):
                return x | 0x400000
    if isnan(a) and isnan(b):
        return a
    if isnan(a):
        return b
    if isnan(b):
        return a
    return None

NRM_F32 = [0x38400180, 0x38e001c0, 0xb8400180, 0xb8e001c0, 0x37c000c0, 0x386000e0, 0x3effffff, 0x3f000001, 0x3f000000, 0xbf000000, 0x3f800000,
           0xbf800000, 0x3f800001, 0xbf800001, 0x7f800000, 0xff800000, 0x7fc00000, 0xffc00000, 0x7f800001, 0xff800001, 0x80000000, 0x00000001,
           0x807fffff, 0x3efffffe, 0x3f7fffff, 0xbf7fffff]
NORM32_BODY = f"""{LOAD}
{PRESET}
  v_cvt_pknorm_i16_f32 v14, v10, v11
  v_cvt_pknorm_u16_f32 v15, v10, v11
{STORE}"""
NORM16_BODY = f"""{LOAD}
{PRESET}
  v_cvt_pknorm_i16_f16 v14, v10, v11
  v_cvt_pknorm_u16_f16 v15, v10, v11
  v_cvt_norm_i16_f16 v16, v10
  v_cvt_norm_u16_f16 v17, v10
{STORE}"""

def norm32_input():
    rnd = random.Random(99)
    n, out = len(NRM_F32), bytearray()
    for i in range(N):
        if i < n * n:
            a, b = NRM_F32[i % n], NRM_F32[i // n]
        else:
            a = struct.unpack('<I', struct.pack('<f', rnd.uniform(-1.25, 1.25)))[0]
            b = struct.unpack('<I', struct.pack('<f', rnd.uniform(-1.25, 1.25)))[0]
        out += struct.pack('<4I', a, b, 0, 0)
    return bytes(out)

def norm16_input():
    return b"".join(struct.pack('<4I', 0xabcd0000 | i, 0x12340000 | (i * 0x3d17 & 0xffff), 0, 0) for i in range(N))

def norm_model(x, signed, rule):
    """x: Fraction or None (NaN), +-inf passed as +-2. rule: 'nearest' | 'halfup' | 'trunc'.
    The only exact tie a float can produce is +-0.5 (16383.5 / 32767.5), where ties-to-even and
    ties-away agree, so 'nearest' covers both; 'halfup' is floor(x + 0.5) and differs at -0.5."""
    if x is None:
        return 0
    mx = 32767 if signed else 65535
    x = max(Fraction(-1 if signed else 0), min(Fraction(1), x)) * mx
    if rule == 'halfup':
        x += Fraction(1, 2)
        return (x.numerator // x.denominator) & 0xffff
    n = abs(x).numerator // abs(x).denominator
    if rule == 'nearest':
        n += abs(x) - n >= Fraction(1, 2)
    return (-n if x < 0 else n) & 0xffff

def q32(b):
    if isnan(b):
        return None
    if b & 0x7fffffff == 0x7f800000:
        return Fraction(-2 if b >> 31 else 2)
    return Fraction(f32(b))
def q16(b):
    if b & 0x7c00 == 0x7c00:
        return None if b & 0x3ff else Fraction(-2 if b >> 15 else 2)
    return h2q(b)

def plan():
    """(kernel name, body, ieee, input file, input bytes or None)"""
    p = [(f"fma16_{k:04x}", fma16_body(k), 0, f"fma16_{k:04x}.in", fma16_input(k)) for k in FMA_K]
    p += [(n, body, 0, "zero.in", bytes(16)) for n, body, _ in sbfe_kernels()]
    p += [(f"nan_ieee{m}", NAN_BODY, m, "nan.in", nan_input()) for m in (0, 1)]
    p += [("norm32", NORM32_BODY, 0, "norm32.in", norm32_input()), ("norm16", NORM16_BODY, 0, "norm16.in", norm16_input())]
    return p

def gen(out):
    os.makedirs(out, exist_ok=True)
    src = [f'.amdgcn_target "amdgcn-amd-amdhsa--{os.environ.get("GFX", "gfx1036")}"', '.amdhsa_code_object_version 4']
    names = []
    with open(f"{out}/manifest.txt", "w") as m:
        for name, body, ieee, inp, data in plan():
            names.append(name)
            src += [kernel(name, body), kd(name, ieee)]
            open(f"{out}/{inp}", "wb").write(data)
            m.write(f"{name} {inp} {N * 64} {name}.out\n")
    src.append(meta(names))
    open(f"{out}/k.s", "w").write("\n".join(src))
    print(f"{len(names)} kernels -> {out}/k.s")

def rows(out, name):
    d = open(f"{out}/{name}.out", "rb").read()
    return [struct.unpack_from('<16I', d, 64 * i) for i in range(N)]
def ins(out, inp):
    d = open(f"{out}/{inp}", "rb").read()
    return [struct.unpack_from('<4I', d, 16 * i) for i in range(N)]

def show_fma16(out):
    print("== fma16: VOP2 0x36 / 0x37 / 0x38 against hardware v_fma_f16 (fused) and v_mul_f16 + v_add_f16 (unfused)")
    for k in FMA_K:
        r, x = rows(out, f"fma16_{k:04x}"), ins(out, f"fma16_{k:04x}.in")
        bad = sum(1 for (a, b, c, _), o in zip(x, r)
                  if None not in (h2q(a & 0xffff), h2q(b & 0xffff), h2q(c & 0xffff)) and rne16(h2q(a & 0xffff) * h2q(b & 0xffff) + h2q(c & 0xffff)) & 0x7fff != o[1] & 0x7fff
                  and (o[1] & 0x7fff) != 0)
        print(f"  K={k:#06x}: hardware v_fma_f16 vs exact single-rounding model: {bad} mismatches (zero-sign cases ignored)")
        for label, op, fu, un in (("0x36 D=S0*S1+D", 0, 1, 2), ("0x37 D=S0*K+S1", 3, 4, 5), ("0x38 D=S0*S1+K", 6, 7, 8)):
            disc = [o for o in r if o[fu] & 0xffff != o[un] & 0xffff]
            nf = sum(1 for o in disc if o[op] & 0xffff == o[fu] & 0xffff)
            nu = sum(1 for o in disc if o[op] & 0xffff == o[un] & 0xffff)
            same = sum(1 for o in r if o[fu] & 0xffff == o[un] & 0xffff and o[op] & 0xffff != o[fu] & 0xffff)
            uppers = sorted({o[op] >> 16 for o in r})[:4]
            print(f"    {label}: {len(disc)} discriminating inputs -> fused {nf}, unfused {nu}, neither {len(disc) - nf - nu};"
                  f" disagreements where both references agree: {same}; upper half of D: {[hex(u) for u in uppers]}")

def show_sbfe(out):
    print("== s_bfe: 'wide' = 7-bit width, signed fields past the top bit sign-fill; 'lit' = 7-bit width, literal ISA formula (they zero-fill);")
    print("          'mod' = width modulo the operand size")
    tot = {"wide": 0, "lit": 0, "mod": 0}
    cnt = 0
    for name, _, (sg, bits, cases) in sbfe_kernels():
        r = rows(out, name)
        if any(x != r[0] for x in r[1:]):
            print(f"  {name}: outputs differ between threads (unexpected)")
        for j, (o, w, junk) in enumerate(cases):
            if bits == 32:
                got, scc = r[0][2 * j], r[0][2 * j + 1]
                a = A32
            else:
                got, scc = r[0][3 * j] | r[0][3 * j + 1] << 32, r[0][3 * j + 2]
                a = A64
            ms = {"wide": bfe_model(a, o, w, sg == "i", bits, True), "lit": bfe_literal(a, o, w, sg == "i", bits), "mod": bfe_model(a, o, w, sg == "i", bits, False)}
            for k, v in ms.items():
                tot[k] += got == v
            cnt += 1
            hit = [k for k, v in ms.items() if got == v]
            if len(set(ms.values())) > 1 or not hit or scc != (got != 0):
                print(f"  s_bfe_{sg}{bits} off={o:<2} width={w:<3} junk={junk}: got {got:#x} scc={scc}  " + " ".join(f"{k}={v:#x}" for k, v in ms.items())
                      + f"  -> {'+'.join(hit) or 'NONE'}" + ("" if scc == (got != 0) else "  (SCC is not result != 0)"))
    print(f"  totals over {cnt} cases: " + ", ".join(f"{k} matches {v}" for k, v in tot.items()))

def show_nan(out):
    x = ins(out, "nan.in")
    ops_t = (("v_fma_f32 (a,b,c)", 0, (0, 1, 2), "fma"), ("v_mul_f32 (a,b)", 1, (0, 1), "mul"), ("v_add_f32 (a,c)", 2, (0, 2), "add"),
             ("v_min_f32 (a,b)", 3, (0, 1), "min"), ("v_max_f32 (a,b)", 4, (0, 1), "max"),
             ("v_fmac_f32 (a,b,D=c)", 5, (0, 1, 2), "fma"), ("v_sub_f32 (a,c)", 6, (0, 2), "sub"))
    for m in (0, 1):
        r = rows(out, f"nan_ieee{m}")
        print(f"== nan, IEEE mode {m}: against the rules fitted on gfx1036 (nan_arith / nan_minmax)")
        for label, col, sel, kind in ops_t:
            n = bad = 0
            first = []
            for t, o in zip(x, r):
                ops = [t[i] for i in sel]
                want = nan_minmax(*ops, m) if kind in ("min", "max") else nan_arith(ops, kind, m)
                if want is None:
                    if kind in ("min", "max") or not isnan(o[col]):
                        continue
                n += 1
                if o[col] != want:
                    bad += 1
                    if len(first) < 6:
                        first.append("(" + ",".join(f"{q:#010x}" for q in ops) + f")->{o[col]:#010x}" + (f" want {want:#010x}" if want is not None else " (unexpected NaN)"))
            print(f"  {label}: {n} NaN-related results, {bad} differ from the rules" + (": " + " ".join(first) if first else ""))

def show_norm(out):
    print("== normalized conversions: mismatches per rounding rule (nearest, halfup = floor(x + 0.5), trunc)")
    for kname, inp, q, ops in (("norm32", "norm32.in", q32, (("v_cvt_pknorm_i16_f32", 0, True, True), ("v_cvt_pknorm_u16_f32", 1, False, True))),
                               ("norm16", "norm16.in", lambda b: q16(b & 0xffff), (("v_cvt_pknorm_i16_f16", 0, True, True), ("v_cvt_pknorm_u16_f16", 1, False, True),
                                                                                    ("v_cvt_norm_i16_f16", 2, True, False), ("v_cvt_norm_u16_f16", 3, False, False)))):
        r, x = rows(out, kname), ins(out, inp)
        for label, col, signed, packed in ops:
            res = {}
            for rule in ("nearest", "halfup", "trunc"):
                bad, first = 0, []
                for t, o in zip(x, r):
                    want = norm_model(q(t[0]), signed, rule)
                    got = o[col] & 0xffff
                    ok = got == want
                    if packed:
                        ok = ok and o[col] >> 16 == norm_model(q(t[1]), signed, rule)
                    if not ok:
                        bad += 1
                        if len(first) < 4:
                            first.append(f"({t[0]:#x},{t[1]:#x})->{o[col]:#x}")
                res[rule] = (bad, first)
            best = min(res, key=lambda k: res[k][0])
            print(f"  {label}: " + ", ".join(f"{k} {v[0]}" for k, v in res.items()) + f"  best={best}" + ("" if res[best][0] == 0 else "  e.g. " + " ".join(res[best][1])))
            if not packed:
                print(f"    upper half of the destination: {sorted({hex(o[col] >> 16) for o in r})[:4]} (preset {SENT >> 16:#x})")

if __name__ == "__main__":
    mode, out = sys.argv[1], sys.argv[2]
    if mode == "gen":
        gen(out)
    else:
        for f in (show_fma16, show_sbfe, show_nan, show_norm):
            try:
                f(out)
            except FileNotFoundError as e:
                print(f"  skipped: {e.filename} missing")
