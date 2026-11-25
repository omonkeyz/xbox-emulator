import os, re, struct, subprocess, sys

SUFFIX = re.compile(r"_(e32|e64|sdwa|dpp8|dpp)$")


def dis(cpu, words):
    hexs = " ".join(f"0x{(w >> (8 * k)) & 0xff:02x}" for w in words for k in range(4))
    r = subprocess.run(["llvm-mc-18", "-arch=amdgcn", f"-mcpu={cpu}", "--disassemble"], input=hexs + "\n", capture_output=True, text=True)
    lines = [l.strip() for l in r.stdout.splitlines() if l.strip() and not l.strip().startswith(".")]
    if r.stderr.strip() or not lines:
        return None, r.stderr.strip().splitlines()[0][:100] if r.stderr.strip() else "no output"
    return lines[0], None


def name(text):
    return SUFFIX.sub("", text.split()[0])


BLOCK = re.compile(r"\b(ds_\w+|s_store\w*|s_buffer_store\w*|s_atomic\w*|s_buffer_atomic\w*|s_scratch\w*|s_sendmsg\w*|s_trap|s_sethalt|s_setkill|image_\w+|flat_\w+|global_\w+|scratch_\w+|exp|s_dcache_wb\w*|s_barrier|s_icache_inv|buffer_atomic\w*|buffer_gl\w*|tbuffer_\w+)\b")
ALLOWED_MEM = {"buffer_load_dwordx4", "buffer_store_dwordx4"}

KD = """
.rodata
.p2align 6
.amdhsa_kernel k
  .amdhsa_user_sgpr_kernarg_segment_ptr 1
  .amdhsa_system_sgpr_workgroup_id_x 1
  .amdhsa_next_free_vgpr 256
  .amdhsa_next_free_sgpr 104
  .amdhsa_reserve_vcc 1
  .amdhsa_wavefront_size32 1
  .amdhsa_kernarg_size 8
  .amdhsa_ieee_mode {IEEE}
  .amdhsa_dx10_clamp 1
  .amdhsa_float_denorm_mode_32 {DN32}
  .amdhsa_float_denorm_mode_16_64 {DN16}
.end_amdhsa_kernel
.amdgpu_metadata
---
amdhsa.version: [ 1, 1 ]
amdhsa.kernels:
  - .name: k
    .symbol: k.kd
    .kernarg_segment_size: 8
    .group_segment_fixed_size: 0
    .private_segment_fixed_size: 0
    .kernarg_segment_align: 8
    .wavefront_size: 32
    .sgpr_count: 104
    .vgpr_count: 256
    .max_flat_workgroup_size: 256
    .args:
      - { .size: 8, .offset: 0, .value_kind: global_buffer, .address_space: global }
...
.end_amdgpu_metadata
"""


def build(d, p):
    words = open(f"{d}/{p}.bin", "rb").read()
    n = len(words) // 4
    ws = struct.unpack(f"<{n}I", words)
    body = "\n".join("  .long " + ", ".join(f"0x{w:08x}" for w in ws[i:i + 8]) for i in range(0, n, 8))
    src = f".text\n.globl k\n.p2align 8\n.type k,@function\nk:\n{body}\n" + KD.replace("{DN32}", os.environ.get("SIL_DN32", "3")).replace("{IEEE}", os.environ.get("SIL_IEEE", "0")).replace("{DN16}", os.environ.get("SIL_DN16", "3"))
    b = f"{d}/build/{p}"
    open(f"{b}.s", "w").write(src)
    subprocess.run(["clang-18", "-x", "assembler", "-target", "amdgcn-amd-amdhsa", "-mcpu=gfx1036", "-c", f"{b}.s", "-o", f"{b}.o"], check=True)
    subprocess.run(["ld.lld-18", "-shared", f"{b}.o", "-o", f"{b}.co"], check=True)
    dis = subprocess.run(["llvm-objdump-18", "-d", "--mcpu=gfx1036", f"{b}.o"], capture_output=True, text=True, check=True).stdout
    open(f"{b}.dis", "w").write(dis)
    return dis


def safety(dis):
    seen = 0
    for line in dis.splitlines():
        if "//" not in line:
            continue
        ins = line.split("//")[0].strip()
        if not ins:
            continue
        seen += 1
        if "<unknown>" in line or "invalid" in line.lower():
            return f"gfx1036 disassembler rejects: {line.strip()[:120]}"
        m = ins.split()[0]
        if m in ALLOWED_MEM:
            continue
        if BLOCK.search(m):
            return f"blocked on gfx1036: {m}"
    if seen == 0:
        return "disassembly produced no instructions"
    return None


def run(d, only):
    os.makedirs(f"{d}/build", exist_ok=True)
    outd = os.environ.get("SIL_OUT", "out")
    os.makedirs(f"{d}/{outd}", exist_ok=True)
    man = [l.split(" ", 3) for l in open(f"{d}/manifest.txt").read().splitlines() if l.strip()]
    ok = skip = fail = 0
    for p, cls, n, names in man:
        if only and p not in only:
            continue
        if all(os.path.exists(f"{d}/{outd}/{p}_b{b}.bin") for b in range(4)):
            ok += 1
            continue
        try:
            dis = build(d, p)
        except subprocess.CalledProcessError as e:
            open(f"{d}/{outd}/{p}.skip", "w").write(f"assembly failed: {e}")
            skip += 1
            continue
        why = safety(dis)
        if why:
            open(f"{d}/{outd}/{p}.skip", "w").write(why)
            print(p, "SKIP", why, flush=True)
            skip += 1
            continue
        args = ["timeout", "30", os.path.join(os.path.dirname(os.path.abspath(__file__)), "sweep_silicon_runner"), f"{d}/build/{p}.co"]
        for b in range(4):
            args += [f"{d}/in_{cls}_{b}.bin", f"{d}/{outd}/{p}_b{b}.bin"]
        r = subprocess.run(args, capture_output=True, text=True)
        if r.returncode != 0:
            open(f"{d}/{outd}/{p}.skip", "w").write(f"runner rc={r.returncode} {r.stderr.strip()[:200]}")
            print(p, "FAIL rc", r.returncode, r.stderr.strip()[:200], flush=True)
            fail += 1
            if r.returncode == 124:
                print("timeout: stopping so the GPU can recover", flush=True)
                break
            continue
        ok += 1
        print(p, "ok", cls, n, flush=True)
    print(f"done: {ok} ok, {skip} skipped, {fail} failed", flush=True)


def absent(d):
    out = []
    n = 0
    for line in open(f"{d}/ops.txt"):
        i, m, fam, ws = line.split()
        words = [int(x, 16) for x in ws.split(",")]
        n += 1
        a, ea = dis("gfx1013", words)
        b, eb = dis("gfx1036", words)
        if b is None and a is not None:
            out.append(f"{i} gfx1036 has no {name(a)} (it decodes on gfx1013 only)")
        elif a is not None and name(a) != name(b):
            out.append(f"{i} encodes {name(b)} on gfx1036, {name(a)} on gfx1013")
    open(f"{d}/absent.txt", "w").write("\n".join(out) + ("\n" if out else ""))
    print(f"{n} ops checked, {len(out)} not available as the same instruction on gfx1036")


def main():
    if sys.argv[1] == "absent":
        absent(sys.argv[2])
    else:
        run(sys.argv[2], sys.argv[3:])


main()
