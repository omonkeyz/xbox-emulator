import sys
F32 = ["v_rcp_f32", "v_rcp_iflag_f32", "v_rsq_f32", "v_sqrt_f32", "v_log_f32", "v_exp_f32", "v_sin_f32", "v_cos_f32"]
F16 = ["v_rcp_f16", "v_rsq_f16", "v_sqrt_f16", "v_log_f16", "v_exp_f16", "v_sin_f16", "v_cos_f16"]
F64 = ["v_rcp_f64", "v_rsq_f64", "v_sqrt_f64"]

def body32(op):
    return f"  v_mov_b32 v3, 0\n  {op} v3, v2\n  global_store_dword v[5:6], v3, off"

def kernel(name, op, wide):
    sh = 3 if wide else 2
    if wide == 3:
        gen = "  v_mul_lo_u32 v8, v1, s6\n  v_mov_b32 v9, s7"
        body = f"  {op} v[10:11], v[8:9]\n  global_store_dwordx2 v[5:6], v[10:11], off"
    elif wide == 2:
        gen = "  s_movk_i32 s8, 0x1003\n  v_mul_lo_u32 v9, v1, s8\n  v_add_nc_u32 v9, s6, v9"
        body = f"  v_mov_b32 v8, s7\n  {op} v[10:11], v[8:9]\n  global_store_dwordx2 v[5:6], v[10:11], off"
    elif wide:
        gen = "  v_mov_b32 v9, s6\n  v_mad_u32_u24 v9, v1, 1, v9"
        body = f"  v_mov_b32 v8, s7\n  {op} v[10:11], v[8:9]\n  global_store_dwordx2 v[5:6], v[10:11], off"
    else:
        gen = "  v_mul_lo_u32 v2, v1, s7\n  v_add_nc_u32 v2, s6, v2"
        body = body32(op)
    return f"""
.text
.globl {name}
.p2align 8
.type {name},@function
{name}:
  s_load_dwordx4 s[4:7], s[0:1], 0x0
  v_mov_b32 v31, 0
  s_waitcnt lgkmcnt(0)
  s_lshl_b32 s9, s2, 8
  v_add_nc_u32 v1, s9, v0
{gen}
  v_lshlrev_b32 v4, {sh}, v1
  v_add_co_u32 v5, vcc_lo, s4, v4
  v_add_co_ci_u32_e32 v6, vcc_lo, s5, v31, vcc_lo
{body}
  s_waitcnt vmcnt(0)
  s_endpgm
"""

def kd(name, d32, d1664):
    return f"""
.rodata
.p2align 6
.amdhsa_kernel {name}
  .amdhsa_user_sgpr_kernarg_segment_ptr 1
  .amdhsa_system_sgpr_workgroup_id_x 1
  .amdhsa_next_free_vgpr 32
  .amdhsa_next_free_sgpr 32
  .amdhsa_wavefront_size32 1
  .amdhsa_kernarg_size 16
  .amdhsa_ieee_mode 0
  .amdhsa_dx10_clamp 1
  .amdhsa_float_denorm_mode_32 {d32}
  .amdhsa_float_denorm_mode_16_64 {d1664}
.end_amdhsa_kernel
"""

def meta(names):
    ks = "\n".join(f"""  - .name: {n}
    .symbol: {n}.kd
    .kernarg_segment_size: 16
    .kernarg_segment_align: 8
    .group_segment_fixed_size: 0
    .private_segment_fixed_size: 0
    .wavefront_size: 32
    .sgpr_count: 32
    .vgpr_count: 32
    .max_flat_workgroup_size: 256
    .args:
      - {{ .size: 8, .offset: 0, .value_kind: global_buffer, .address_space: global }}
      - {{ .size: 4, .offset: 8, .value_kind: by_value }}
      - {{ .size: 4, .offset: 12, .value_kind: by_value }}""" for n in names)
    return f".amdgpu_metadata\n---\namdhsa.version: [ 1, 1 ]\namdhsa.kernels:\n{ks}\n...\n.end_amdgpu_metadata\n"

names, src = [], []
for d in (0, 3):
    for op in F32 + F16 + F64:
        n = f"{op}_d{d}"
        names.append(n)
        src.append(kernel(n, op, op in F64))
        src.append(kd(n, d if op in F32 else 3, d if op not in F32 else 3))
    for op in F64:
        n = f"{op}x_d{d}"
        names.append(n)
        src.append(kernel(n, op, 2))
        src.append(kd(n, 3, d))
        n = f"{op}m_d{d}"
        names.append(n)
        src.append(kernel(n, op, 3))
        src.append(kd(n, 3, d))
open(sys.argv[1], "w").write("\n".join(src) + "\n" + meta(names))
print(len(names), "kernels")
