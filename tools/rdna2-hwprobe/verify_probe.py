import random, struct, sys

OPS = [
    ("v_fmac_f16", "v_fmac_f16 v10, v1, v2"),
    ("v_fmamk_f16", "v_fmamk_f16 v10, v1, 0x3555, v2"),
    ("v_fmaak_f16", "v_fmaak_f16 v10, v1, v2, 0xc248"),
    ("v_pk_lshlrev_b16", "v_pk_lshlrev_b16 v10, v1, v2"),
    ("v_pk_lshrrev_b16", "v_pk_lshrrev_b16 v10, v1, v2"),
    ("v_cvt_pknorm_i16_f32", "v_cvt_pknorm_i16_f32 v10, v1, v2"),
    ("v_cvt_pknorm_u16_f32", "v_cvt_pknorm_u16_f32 v10, v1, v2"),
    ("v_cvt_pknorm_i16_f16", "v_cvt_pknorm_i16_f16 v10, v1, v2"),
    ("v_cvt_pknorm_u16_f16", "v_cvt_pknorm_u16_f16 v10, v1, v2"),
    ("v_cvt_norm_i16_f16", "v_cvt_norm_i16_f16 v10, v1"),
    ("v_cvt_norm_u16_f16", "v_cvt_norm_u16_f16 v10, v1"),
    ("s_bfe_u32", "s_bfe_u32 s24, s20, s21"),
    ("s_bfe_i32", "s_bfe_i32 s24, s20, s21"),
    ("s_bfe_u64", "s_bfe_u64 s[24:25], s[20:21], s22"),
    ("s_bfe_i64", "s_bfe_i64 s[24:25], s[20:21], s22"),
]

F16 = [0x0000, 0x8000, 0x3c00, 0xbc00, 0x0001, 0x83ff, 0x0400, 0x7bff, 0xfbff, 0x7c00, 0xfc00, 0x7e00, 0x7e01, 0x3555, 0x4248, 0xc900, 0x1400, 0x6000]


def quiet(h):
    return h | 0x200 if h & 0x7C00 == 0x7C00 and h & 0x3FF else h


def kernel(name, instr):
    return f"""
.text
.globl {name}
.p2align 8
.type {name},@function
{name}:
  s_load_dwordx4 s[4:7], s[0:1], 0x0
  s_waitcnt lgkmcnt(0)
  s_lshl_b32 s8, s2, 4
  s_load_dwordx4 s[20:23], s[4:5], s8
  s_waitcnt lgkmcnt(0)
  v_mov_b32 v1, s20
  v_mov_b32 v2, s21
  v_mov_b32 v3, s22
  v_mov_b32 v10, s23
  s_mov_b32 s24, 0
  s_mov_b32 s25, 0
  {instr}
  s_cselect_b32 s26, 1, 0
  v_mov_b32 v11, s24
  v_mov_b32 v12, s25
  v_mov_b32 v13, s26
  v_mov_b32 v14, s8
  v_cmp_eq_u32 vcc_lo, 0, v0
  s_and_saveexec_b32 s9, vcc_lo
  global_store_dwordx4 v14, v[10:13], s[6:7]
  s_waitcnt vmcnt(0)
  s_endpgm
.rodata
.p2align 6
.amdhsa_kernel {name}
  .amdhsa_user_sgpr_kernarg_segment_ptr 1
  .amdhsa_system_sgpr_workgroup_id_x 1
  .amdhsa_next_free_vgpr 32
  .amdhsa_next_free_sgpr 40
  .amdhsa_wavefront_size32 1
  .amdhsa_kernarg_size 16
  .amdhsa_ieee_mode 0
  .amdhsa_dx10_clamp 1
  .amdhsa_float_denorm_mode_32 3
  .amdhsa_float_denorm_mode_16_64 3
.end_amdhsa_kernel
"""


STORES = [
    ("bs_x", "buffer_store_format_x v4, v1, s[12:15], 0 idxen"),
    ("bs_xy", "buffer_store_format_xy v[4:5], v1, s[12:15], 0 idxen"),
    ("bs_xyz", "buffer_store_format_xyz v[4:6], v1, s[12:15], 0 idxen"),
]

FORMATS = [75, 77, 72, 62, 20]


def store_kernel(name, instr):
    return f"""
.text
.globl {name}
.p2align 8
.type {name},@function
{name}:
  s_load_dwordx4 s[4:7], s[0:1], 0x0
  s_waitcnt lgkmcnt(0)
  s_lshl_b32 s8, s2, 4
  s_load_dwordx4 s[20:23], s[4:5], s8
  s_waitcnt lgkmcnt(0)
  s_mov_b32 s12, s6
  s_and_b32 s13, s7, 0xffff
  s_or_b32 s13, s13, 0x100000
  s_mov_b32 s14, 0x7fffffff
  s_lshl_b32 s15, s23, 12
  s_or_b32 s15, s15, 0x31000fac
  v_mov_b32 v1, s2
  v_mov_b32 v4, s20
  v_mov_b32 v5, s21
  v_mov_b32 v6, s22
  v_cmp_eq_u32 vcc_lo, 0, v0
  s_and_saveexec_b32 s9, vcc_lo
  {instr}
  s_waitcnt vmcnt(0)
  s_endpgm
""" + descriptor(name, 0)


def lds_kernel(name):
    return f"""
.text
.globl {name}
.p2align 8
.type {name},@function
{name}:
  s_load_dwordx4 s[4:7], s[0:1], 0x0
  s_waitcnt lgkmcnt(0)
  s_lshl_b32 s8, s2, 4
  s_load_dwordx4 s[20:23], s[4:5], s8
  s_waitcnt lgkmcnt(0)
  v_lshlrev_b32 v1, 2, v0
  v_mov_b32 v4, s20
  v_mov_b32 v5, s21
  v_add_nc_u32 v2, s22, v1
  ds_write_b32 v1, v4
  s_waitcnt lgkmcnt(0)
  ds_write_b16_d16_hi v2, v5
  s_waitcnt lgkmcnt(0)
  ds_read_b32 v10, v1
  s_waitcnt lgkmcnt(0)
  v_mov_b32 v11, s22
  v_mov_b32 v12, s20
  v_mov_b32 v13, s21
  v_mov_b32 v14, s8
  v_cmp_eq_u32 vcc_lo, 0, v0
  s_and_saveexec_b32 s9, vcc_lo
  global_store_dwordx4 v14, v[10:13], s[6:7]
  s_waitcnt vmcnt(0)
  s_endpgm
""" + descriptor(name, 1024)


def descriptor(name, lds):
    return f"""
.rodata
.p2align 6
.amdhsa_kernel {name}
  .amdhsa_user_sgpr_kernarg_segment_ptr 1
  .amdhsa_system_sgpr_workgroup_id_x 1
  .amdhsa_next_free_vgpr 32
  .amdhsa_next_free_sgpr 40
  .amdhsa_wavefront_size32 1
  .amdhsa_kernarg_size 16
  .amdhsa_group_segment_fixed_size {lds}
.end_amdhsa_kernel
"""


def meta(names, lds_names=()):
    ks = "\n".join(f"""  - .name: {n}
    .symbol: {n}.kd
    .kernarg_segment_size: 16
    .group_segment_fixed_size: {1024 if n in lds_names else 0}
    .private_segment_fixed_size: 0
    .kernarg_segment_align: 8
    .wavefront_size: 32
    .sgpr_count: 40
    .vgpr_count: 32
    .max_flat_workgroup_size: 32
    .args:
      - {{ .size: 8, .offset: 0, .value_kind: global_buffer, .address_space: global }}
      - {{ .size: 8, .offset: 8, .value_kind: global_buffer, .address_space: global }}""" for n in names)
    return f".amdgpu_metadata\n---\namdhsa.version: [ 1, 1 ]\namdhsa.kernels:\n{ks}\n...\n.end_amdgpu_metadata\n"


def inputs(name, n, rng):
    out = []
    for i in range(n):
        r = [rng.getrandbits(32) for _ in range(4)]
        if name.endswith("_f16"):
            a, b, c = F16[i % len(F16)], F16[(i * 7 + 3) % len(F16)], F16[(i * 5 + 1) % len(F16)]
            if i >= 2 * len(F16):
                a, b, c = rng.getrandbits(16), rng.getrandbits(16), rng.getrandbits(16)
            a, b, c = (quiet(x) for x in (a, b, c))
            r = [(r[0] & 0xFFFF0000) | a, (r[1] & 0xFFFF0000) | b, r[2], (r[3] & 0xFFFF0000) | c]
        elif name.endswith("_f32") and "norm" in name:
            def f32v(j):
                scale = 32767.0 if "i16" in name else 65535.0
                k = rng.randrange(-40000, 70000)
                choice = j % 6
                if choice == 0:
                    v = (k + 0.5) / scale
                elif choice == 1:
                    v = k / scale
                elif choice == 2:
                    v = rng.uniform(-1.5, 1.5)
                elif choice == 3:
                    return [0x00000000, 0x80000000, 0x3F800000, 0xBF800000, 0x7F800000, 0xFF800000, 0x7FC00000, 0x00000001, 0x3F000000, 0x40000000][j // 6 % 10]
                else:
                    b = struct.unpack("<I", struct.pack("<f", (k + 0.5) / scale))[0]
                    return b + (1 if choice == 4 else -1)
                return struct.unpack("<I", struct.pack("<f", v))[0]
            r[0], r[1] = f32v(2 * i), f32v(2 * i + 1)
        elif "norm" in name:
            def f16v(j):
                if j % 3 == 0:
                    return F16[j // 3 % len(F16)] if F16[j // 3 % len(F16)] != 0x7D01 else 0x7E01
                return quiet(rng.getrandbits(16) & 0xBFFF if j % 3 == 1 else rng.getrandbits(16))
            r[0] = (r[0] & 0xFFFF0000) | f16v(2 * i)
            r[1] = (r[1] & 0xFFFF0000) | f16v(2 * i + 1)
        elif name.startswith("v_pk_"):
            if i < 40:
                r[0] = ((i % 20) << 16) | (rng.getrandbits(12) << 4) | ((i * 3) % 20)
        elif name.startswith("s_bfe"):
            off, width = i % 40, [0, 1, 7, 16, 31, 32, 33, 63, 64, 65, 127][i % 11]
            ctl = (width << 16) | off | (rng.getrandbits(9) << 23 if i % 3 == 0 else 0)
            if name.endswith("64"):
                r[2] = ctl
            else:
                r[1] = ctl
        out.append(r)
    return out


def main():
    asm_out, inp_dir, n = sys.argv[1], sys.argv[2], int(sys.argv[3])
    rng = random.Random(20261122)
    src, names = [], []
    for name, instr in OPS:
        k = "hv_" + name
        names.append(k)
        src.append(kernel(k, instr))
        rows = inputs(name, n, rng)
        open(f"{inp_dir}/{name}.in", "wb").write(b"".join(struct.pack("<4I", *r) for r in rows))
    for name, instr in STORES:
        names.append(name)
        src.append(store_kernel(name, instr))
    open(f"{inp_dir}/stores.in", "wb").write(b"".join(struct.pack("<4I", 0x11223344, 0x55667788, 0x99AABBCC, f) for f in FORMATS))
    names.append("ds_hi")
    src.append(lds_kernel("ds_hi"))
    lds_rng = random.Random(7)
    open(f"{inp_dir}/ds_hi.in", "wb").write(b"".join(struct.pack("<4I", lds_rng.getrandbits(32), lds_rng.getrandbits(32), (i & 1) * 2, 0) for i in range(n)))
    open(asm_out, "w").write("\n".join(src) + "\n" + meta(names, ("ds_hi",)))
    for name, instr in OPS:
        print(name, "|", instr)


if __name__ == "__main__":
    main()
