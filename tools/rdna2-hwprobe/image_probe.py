import struct, sys

ATOMS = ["swap", "cmpswap", "add", "sub", "smin", "umin", "smax", "umax", "and", "or", "xor", "inc", "dec"]
INIT = [5, 0x10, 0x80000000, 0xFFFFFFFF, 0x20, 0, 0x7FFFFFFF, 0x12345678]
TUPLES = [(0, 5, 0xFFFFFFF0), (1, 0x10, 0x10), (2, 0x80000000, 0x80000000), (3, 7, 0xFFFFFFFF), (4, 0x20, 1), (5, 0, 0), (6, 0xFFFFFFFF, 0x7FFFFFFF), (7, 0x12345678, 3)]


def kernel(name, op, glc):
    dm = "0x3" if op == "cmpswap" else "0x1"
    data = "v[4:5]" if op == "cmpswap" else "v4"
    g = " glc" if glc else ""
    return f"""
.text
.globl {name}
.p2align 8
.type {name},@function
{name}:
  s_load_dwordx4 s[4:7], s[0:1], 0x0
  s_waitcnt lgkmcnt(0)
  s_lshl_b32 s8, s2, 6
  s_load_dwordx16 s[20:35], s[4:5], s8
  s_waitcnt lgkmcnt(0)
  s_lshl_b32 s9, s2, 8
  s_add_u32 s10, s6, s9
  s_addc_u32 s11, s7, 0
  s_lshr_b64 s[36:37], s[10:11], 8
  s_mov_b32 s12, s36
  s_and_b32 s13, s37, 0xff
  s_or_b32 s13, s13, 0xc1400000
  s_mov_b32 s14, 0x80000001
  s_mov_b32 s15, 0x90000fac
  s_mov_b32 s16, 0
  s_mov_b32 s17, 0
  s_mov_b32 s18, 0
  s_mov_b32 s19, 0
  v_mov_b32 v20, s24
  v_mov_b32 v21, s25
  v_mov_b32 v22, s26
  v_mov_b32 v23, s27
  v_mov_b32 v24, s28
  v_mov_b32 v25, s29
  v_mov_b32 v26, s30
  v_mov_b32 v27, s31
  v_mov_b32 v30, 0
  v_cmp_eq_u32 vcc_lo, 0, v0
  s_and_saveexec_b32 s38, vcc_lo
  global_store_dwordx4 v30, v[20:23], s[10:11] glc dlc
  global_store_dwordx4 v30, v[24:27], s[10:11] offset:16 glc dlc
  s_waitcnt vmcnt(0)
  s_waitcnt_vscnt null, 0x0
  v_mov_b32 v1, s20
  v_mov_b32 v2, 0
  v_mov_b32 v4, s21
  v_mov_b32 v5, s22
  image_atomic_{op} {data}, v[1:2], s[12:19] dmask:{dm} dim:SQ_RSRC_IMG_2D unorm{g}
  s_waitcnt vmcnt(0)
  global_load_dwordx4 v[10:13], v30, s[10:11] glc dlc
  global_load_dwordx4 v[14:17], v30, s[10:11] offset:16 glc dlc
  s_waitcnt vmcnt(0)
  global_store_dwordx4 v30, v[10:13], s[10:11] offset:64
  global_store_dwordx4 v30, v[14:17], s[10:11] offset:80
  global_store_dword v30, v4, s[10:11] offset:96
  s_waitcnt vmcnt(0)
  s_endpgm
.rodata
.p2align 6
.amdhsa_kernel {name}
  .amdhsa_user_sgpr_kernarg_segment_ptr 1
  .amdhsa_system_sgpr_workgroup_id_x 1
  .amdhsa_next_free_vgpr 40
  .amdhsa_next_free_sgpr 48
  .amdhsa_wavefront_size32 1
  .amdhsa_kernarg_size 16
.end_amdhsa_kernel
"""


def meta(names):
    ks = "\n".join(f"""  - .name: {n}
    .symbol: {n}.kd
    .kernarg_segment_size: 16
    .group_segment_fixed_size: 0
    .private_segment_fixed_size: 0
    .kernarg_segment_align: 8
    .wavefront_size: 32
    .sgpr_count: 48
    .vgpr_count: 40
    .max_flat_workgroup_size: 32
    .args:
      - {{ .size: 8, .offset: 0, .value_kind: global_buffer, .address_space: global }}
      - {{ .size: 8, .offset: 8, .value_kind: global_buffer, .address_space: global }}""" for n in names)
    return f".amdgpu_metadata\n---\namdhsa.version: [ 1, 1 ]\namdhsa.kernels:\n{ks}\n...\n.end_amdgpu_metadata\n"


MODEL = {
    "swap": lambda m, s, c: s,
    "cmpswap": lambda m, s, c: s if m == c else m,
    "add": lambda m, s, c: (m + s) & 0xFFFFFFFF,
    "sub": lambda m, s, c: (m - s) & 0xFFFFFFFF,
    "smin": lambda m, s, c: min(signed(m), signed(s)) & 0xFFFFFFFF,
    "umin": lambda m, s, c: min(m, s),
    "smax": lambda m, s, c: max(signed(m), signed(s)) & 0xFFFFFFFF,
    "umax": lambda m, s, c: max(m, s),
    "and": lambda m, s, c: m & s,
    "or": lambda m, s, c: m | s,
    "xor": lambda m, s, c: m ^ s,
    "inc": lambda m, s, c: 0 if m >= s else m + 1,
    "dec": lambda m, s, c: s if m == 0 or m > s else m - 1,
}


def signed(v):
    return v - (1 << 32) if v >> 31 else v


def check(out_dir):
    total = bad = 0
    for op in ATOMS:
        for glc in (1, 0):
            d = open(f"{out_dir}/{op}_{glc}.bin", "rb").read()
            for i, (x, s, c) in enumerate(TUPLES):
                mem = list(struct.unpack_from("<8I", d, 256 * i + 64))
                ret = struct.unpack_from("<I", d, 256 * i + 96)[0]
                want = list(INIT)
                want[x] = MODEL[op](INIT[x], s, c)
                total += 1
                if mem != want or ret != (INIT[x] if glc else s):
                    bad += 1
                    print(f"image_atomic_{op} glc={glc} case {i}: memory {[hex(v) for v in mem]} returned {ret:#x}")
    print(f"{total - bad}/{total} cases match")


OPCODES = {"swap": 0x0F, "cmpswap": 0x10, "add": 0x11, "sub": 0x12, "smin": 0x14, "umin": 0x15, "smax": 0x16, "umax": 0x17, "and": 0x18, "or": 0x19, "xor": 0x1A, "inc": 0x1B, "dec": 0x1C}


def rust(out_dir, out, source):
    lines = [f'pub const SOURCE: &str = "{source}";', ""]
    lines.append("pub const ATOMICS: &[(&str, u32, bool, u32, u32, u32, [u32; 8], u32)] = &[")
    for op in ATOMS:
        for glc in (1, 0):
            d = open(f"{out_dir}/{op}_{glc}.bin", "rb").read()
            for i, (x, s, c) in enumerate(TUPLES):
                mem = struct.unpack_from("<8I", d, 256 * i + 64)
                ret = struct.unpack_from("<I", d, 256 * i + 96)[0]
                texels = ", ".join(f"{v:#010x}" for v in mem)
                lines.append(f'    ("image_atomic_{op}", {OPCODES[op]:#04x}, {"true" if glc else "false"}, {x}, {s:#010x}, {c:#010x}, [{texels}], {ret:#010x}),')
    lines.append("];")
    open(out, "w").write("\n".join(lines) + "\n")


def main():
    if sys.argv[1] == "check":
        check(sys.argv[2])
        return
    if sys.argv[1] == "rust":
        rust(sys.argv[2], sys.argv[3], sys.argv[4])
        return
    asm_out, inp = sys.argv[1], sys.argv[2]
    src, names = [], []
    for op in ATOMS:
        for glc in (1, 0):
            n = f"ia_{op}_{glc}"
            names.append(n)
            src.append(kernel(n, op, glc))
    open(asm_out, "w").write("\n".join(src) + "\n" + meta(names))
    rows = b""
    for x, s, c in TUPLES:
        rows += struct.pack("<16I", x, s, c, 0, *INIT, 0, 0, 0, 0)
    open(inp, "wb").write(rows)


if __name__ == "__main__":
    main()
