use super::ds::{cs_prog, ds, dv, ev};
use super::*;
use tombox_xshader::gds_abi::{TB_GDS_HLSL_DECL, TB_GDS_TITLE_DWORDS};

const GDS_BYTES: u64 = 0x1_0000;
const T: u32 = 64;

fn gds_op(op: u32, offset: u32, addr: u32, d0: u32, d1: u32, vdst: u32) -> [u32; 2] {
    let mut w = ds(op, offset, addr, d0, d1, vdst);
    w[0] |= 1 << 17;
    w
}

fn exec_gds(words: &[u32], gds_init: &[u32]) -> Result<(Vec<u32>, Vec<u32>), String> {
    exec_gds_threads(words, gds_init, T)
}

fn exec_gds_threads(words: &[u32], gds_init: &[u32], threads: u32) -> Result<(Vec<u32>, Vec<u32>), String> {
    let ex = cs_extract(words, threads);
    let t = translate(&ex)?;
    if !t.hlsl.contains(TB_GDS_HLSL_DECL) {
        return Err(format!("no GDS binding in\n{}", t.hlsl));
    }
    let hlsl = t.hlsl.replace(TB_GDS_HLSL_DECL, "RWByteAddressBuffer tb_gds : register(u1);");
    let code = dxc::compile(&hlsl, &dxc::profile_for(t.profile)).map_err(|e| format!("dxc: {e}\n{hlsl}"))?;
    let m = CTX.get_or_init(|| Mutex::new(unsafe { ctx_new() }));
    let mut c = m.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { run(&mut c, &hlsl, &code, gds_init) }.map_err(|e| format!("{e}\n{hlsl}"))
}

unsafe fn run(c: &mut Ctx, hlsl: &str, code: &[u8], gds_init: &[u32]) -> Result<(Vec<u32>, Vec<u32>), String> {
    let g = &c.gpu;
    let blob = ser_ranges(&[vec![[1, 8, 0, 0, 0xffff_ffff]]])?;
    let mut rs: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(g.device, 16, fn(u32, *const u8, usize, *const Guid, *mut *mut c_void) -> i32, 0, blob.as_ptr(), blob.len(), &d3d::IID_ROOTSIG, &mut rs);
    if hr < 0 {
        return Err(format!("CreateRootSignature {hr:#x}"));
    }
    let mut d = [0u8; 56];
    d[0..8].copy_from_slice(&(rs as u64).to_le_bytes());
    d[8..16].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    d[16..24].copy_from_slice(&(code.len() as u64).to_le_bytes());
    let mut pso: *mut c_void = core::ptr::null_mut();
    let hr = vcall!(g.device, 11, fn(*const u8, *const Guid, *mut *mut c_void) -> i32, d.as_ptr(), &d3d::IID_PSO, &mut pso);
    if hr < 0 {
        d3d::release(rs);
        return Err(format!("CreateComputePipelineState hr={:#010x}{}", hr as u32, g.messages()));
    }
    let (vh, vcpu, vgpu, vinc) = g.heap(0, 8, true)?;
    let ubuf = g.buffer(CS_BYTES, 1, d3d::COPY_DEST, true)?;
    let gbuf = g.buffer(GDS_BYTES, 1, d3d::COPY_DEST, true)?;
    let up = g.buffer(CS_BYTES + GDS_BYTES, 2, d3d::GENERIC_READ, false)?;
    let rb = g.buffer(CS_BYTES + GDS_BYTES, 3, d3d::COPY_DEST, false)?;
    let mut bytes = vec![0u8; (CS_BYTES + GDS_BYTES) as usize];
    for (i, w) in gds_init.iter().enumerate() {
        let o = CS_BYTES as usize + 4 * i;
        bytes[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    Gpu::map_write(up, &bytes);
    let n: u32 = hlsl.split("struct TbElem_ubuf0 { uint w[").nth(1).and_then(|s| s.split(']').next()).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut ud = [0u8; 40];
    ud[4..8].copy_from_slice(&1u32.to_le_bytes());
    ud[16..20].copy_from_slice(&((CS_BYTES as u32) / (n * 4)).to_le_bytes());
    ud[20..24].copy_from_slice(&(n * 4).to_le_bytes());
    vcall!(g.device, 19, fn(*mut c_void, *mut c_void, *const u8, usize) -> (), ubuf, core::ptr::null_mut(), ud.as_ptr(), vcpu);
    let mut gd = [0u8; 40];
    gd[0..4].copy_from_slice(&39u32.to_le_bytes());
    gd[4..8].copy_from_slice(&1u32.to_le_bytes());
    gd[16..20].copy_from_slice(&((GDS_BYTES / 4) as u32).to_le_bytes());
    gd[32..36].copy_from_slice(&1u32.to_le_bytes());
    vcall!(g.device, 19, fn(*mut c_void, *mut c_void, *const u8, usize) -> (), gbuf, core::ptr::null_mut(), gd.as_ptr(), vcpu + vinc as usize);
    c.gpu.begin();
    let l = c.gpu.list;
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), ubuf, 0, up, 0, CS_BYTES);
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), gbuf, 0, up, CS_BYTES, GDS_BYTES);
    c.gpu.barrier(ubuf, d3d::COPY_DEST, 8);
    c.gpu.barrier(gbuf, d3d::COPY_DEST, 8);
    let heaps = [vh];
    vcall!(l, 28, fn(u32, *const *mut c_void) -> (), 1, heaps.as_ptr());
    vcall!(l, 29, fn(*mut c_void) -> (), rs);
    vcall!(l, 31, fn(u32, u64) -> (), 0, vgpu);
    vcall!(l, 25, fn(*mut c_void) -> (), pso);
    vcall!(l, 14, fn(u32, u32, u32) -> (), 1, 1, 1);
    c.gpu.barrier(ubuf, 8, d3d::COPY_SOURCE);
    c.gpu.barrier(gbuf, 8, d3d::COPY_SOURCE);
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), rb, 0, ubuf, 0, CS_BYTES);
    vcall!(l, 15, fn(*mut c_void, u64, *mut c_void, u64, u64) -> (), rb, CS_BYTES, gbuf, 0, GDS_BYTES);
    let r = c.gpu.submit();
    let out = r.and_then(|_| Gpu::map_read(rb, (CS_BYTES + GDS_BYTES) as usize).ok_or_else(|| "map".to_string())).map(|b| {
        let w: Vec<u32> = b.chunks(4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).collect();
        let k = (CS_BYTES / 4) as usize;
        (w[..k].to_vec(), w[k..].to_vec())
    });
    for k in [pso, rs, vh, ubuf, gbuf, up, rb] {
        d3d::release(k);
    }
    out
}

fn gi(k: usize) -> u32 {
    (k as u32).wrapping_mul(0x0101_0101) ^ 0x7000_0abc
}

fn init() -> Vec<u32> {
    (0..(GDS_BYTES / 4) as usize).map(gi).collect()
}

const BASE: u32 = 0x100;
const SIZE: u32 = 0x1000;
const OFF: u32 = 8;

fn m0(base: u32, size: u32) -> u32 {
    base << 16 | size
}

fn slot(t: u32) -> usize {
    ((BASE + t * 4 + OFF) / 4) as usize
}

fn prog(m0: u32, body: &[u32], outs: &[u32]) -> Vec<u32> {
    let mut b = vec![sop1(3, 124, LIT), m0];
    b.extend_from_slice(body);
    cs_prog(&b, outs)
}

type F = fn(u32, u32, u32) -> u32;

#[test]
fn ds_gds_atomics() {
    let ops: [(&str, u32, F); 14] = [
        ("ds_add_u32", 0x00, |o, a, _| o.wrapping_add(a)),
        ("ds_sub_u32", 0x01, |o, a, _| o.wrapping_sub(a)),
        ("ds_rsub_u32", 0x02, |o, a, _| a.wrapping_sub(o)),
        ("ds_inc_u32", 0x03, |o, a, _| if o >= a { 0 } else { o + 1 }),
        ("ds_dec_u32", 0x04, |o, a, _| if o == 0 || o > a { a } else { o - 1 }),
        ("ds_min_i32", 0x05, |o, a, _| (o as i32).min(a as i32) as u32),
        ("ds_max_i32", 0x06, |o, a, _| (o as i32).max(a as i32) as u32),
        ("ds_min_u32", 0x07, |o, a, _| o.min(a)),
        ("ds_max_u32", 0x08, |o, a, _| o.max(a)),
        ("ds_and_b32", 0x09, |o, a, _| o & a),
        ("ds_or_b32", 0x0a, |o, a, _| o | a),
        ("ds_xor_b32", 0x0b, |o, a, _| o ^ a),
        ("ds_cmpst_b32", 0x10, |o, a, b| if o == a { b } else { o }),
        ("ds_write_b32", 0x0d, |_, a, _| a),
    ];
    let mut fails = Vec::new();
    let mut ok = Vec::new();
    let g0 = init();
    for (m, op, f) in ops {
        for rtn in [false, true] {
            if rtn && op == 0x0d {
                continue;
            }
            let code = if rtn { op + 0x20 } else { op };
            let words = prog(m0(BASE, SIZE), &gds_op(code, OFF, 1, 2, 4, 10), &[10]);
            let name = if rtn { m.replacen("_u32", "_rtn_u32", 1).replacen("_i32", "_rtn_i32", 1).replacen("_b32", "_rtn_b32", 1) } else { m.to_string() };
            let isa: Vec<u8> = gds_op(code, OFF, 1, 2, 4, 10).iter().flat_map(|w| w.to_le_bytes()).collect();
            let dec = tombox_xshader::rdna2::disassemble(&isa)[0].mnemonic.clone();
            if dec != name {
                fails.push(format!("{name}: decodes as {dec}"));
                continue;
            }
            let mut gs = g0.clone();
            if op == 0x10 {
                for t in (0..T).step_by(2) {
                    gs[slot(t)] = dv(t);
                }
            }
            let rt = super::runtime_words(&words, &gds_op(code, OFF, 1, 2, 4, 10), vop2(0x16, 200, ic(31), 0));
            assert!(rt.is_some(), "{name}: no runtime-input variant");
            let mut good = true;
            for (tag, w) in [("", Some(words.clone())), (" (runtime inputs)", rt)] {
                let Some(w) = w else { continue };
                match exec_gds(&w, &gs) {
                    Ok((mem, gds)) => {
                        let mut bad = Vec::new();
                        for t in 0..T {
                            let s = slot(t);
                            let want = f(gs[s], dv(t), ev(t));
                            if gds[s] != want {
                                bad.push(format!("gds[{s}] {:08x} want {want:08x}", gds[s]));
                            }
                            if rtn && mem[t as usize] != gs[s] {
                                bad.push(format!("rtn[{t}] {:08x} want {:08x}", mem[t as usize], gs[s]));
                            }
                        }
                        if !bad.is_empty() {
                            good = false;
                            fails.push(format!("{name}{tag}: {}", bad.into_iter().take(4).collect::<Vec<_>>().join(", ")));
                        }
                    }
                    Err(e) => {
                        good = false;
                        fails.push(format!("{name}{tag}: {}", e.lines().take(3).collect::<Vec<_>>().join(" | ")));
                    }
                }
            }
            if good {
                ok.push(name);
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
    println!("ds_gds_atomics: {} mnemonics verified", ok.len());
}

#[test]
fn ds_gds_shared_counters() {
    let g0 = init();
    let add = prog(m0(0, SIZE), &gds_op(0x00, 0x40, 13, 2, 0, 0), &[]);
    let add = {
        let mut w = vec![vop1(1, 13, ic(0))];
        w.extend_from_slice(&add);
        w
    };
    let (_, gds) = exec_gds(&add, &g0).unwrap();
    let sum = (0..T).fold(g0[0x10], |a, t| a.wrapping_add(dv(t)));
    assert_eq!(gds[0x10], sum);
    for (op, delta) in [(0x3eu32, 1u32), (0x3d, 0xffff_ffff)] {
        let isa: Vec<u8> = gds_op(op, 0, 0, 0, 0, 10).iter().flat_map(|w| w.to_le_bytes()).collect();
        let name = tombox_xshader::rdna2::disassemble(&isa)[0].mnemonic.clone();
        let words = prog(m0(0x220, SIZE), &gds_op(op, 0, 0, 0, 0, 10), &[10]);
        let (_, gds) = exec_gds(&words, &g0).unwrap();
        let s = (0x220 / 4) as usize;
        assert_eq!(gds[s], g0[s].wrapping_add(delta.wrapping_mul(T)), "{name}");
    }
    let lim = TB_GDS_TITLE_DWORDS * 4;
    let words = prog(m0(lim - OFF, SIZE), &gds_op(0x2d, OFF, 1, 2, 0, 10), &[10]);
    let (mem, gds) = exec_gds(&words, &g0).unwrap();
    assert!(mem[..T as usize].iter().all(|&v| v == 0));
    assert_eq!(gds, g0);
    let words = prog(m0(BASE, SIZE), &gds_op(0x2d, OFF, 1, 2, 0, 10), &[10]);
    let (mem, gds) = exec_gds(&words, &g0).unwrap();
    for t in 0..T {
        assert_eq!((gds[slot(t)], mem[t as usize]), (dv(t), g0[slot(t)]));
    }
    let words = prog(m0(BASE, OFF + 32 * 4), &gds_op(0x2d, OFF, 1, 2, 0, 10), &[10]);
    let (mem, gds) = exec_gds(&words, &g0).unwrap();
    for t in 0..T {
        let want = if t < 32 { (dv(t), g0[slot(t)]) } else { (g0[slot(t)], 0) };
        assert_eq!((gds[slot(t)], mem[t as usize]), want, "lane {t}");
    }
    let mut rd = gds_op(0x36, OFF, 1, 0, 0, 10).to_vec();
    rd.extend_from_slice(&gds_op(0x0d, OFF + 0x400, 1, 2, 0, 0));
    let (mem, gds) = exec_gds(&prog(m0(BASE, SIZE), &rd, &[10]), &g0).unwrap();
    for t in 0..T {
        assert_eq!(mem[t as usize], g0[slot(t)]);
    }
    for t in 0..T {
        assert_eq!(gds[slot(t) + 256], dv(t));
    }
    for op in [0x19u32, 0x1d, 0x3f] {
        let words = prog(0, &gds_op(op, 0, 1, 2, 0, 10), &[10]);
        assert!(translate(&cs_extract(&words, T)).is_err(), "op {op:#x} must be refused");
    }
}

const SENT: u32 = 0x5eed_0bad;

fn counter_cases() -> Vec<(&'static str, u32, Vec<u32>, fn(u32) -> bool)> {
    vec![
        ("all", T, vec![], |_| true),
        ("t>20", T, vec![vopc(0xd1, ic(20), 0)], |t| t > 20),
        ("odd", T, vec![vop2(0x1b, 20, ic(1), 0), vopc(0xd5, ic(0), 20)], |t| t & 1 == 1),
        ("lane 5 of wave 1", T, vec![vopc(0xd2, LIT, 0), 37], |t| t == 37),
        ("partial final wave", 45, vec![], |_| true),
        ("empty mask", 45, vec![vopc(0xd2, LIT, 0), 99], |_| false),
    ]
}

#[test]
fn ds_gds_append_consume() {
    // The last title-owned dword is adjacent to the reserved counter table.
    let s = TB_GDS_TITLE_DWORDS as usize - 1;
    for (op, delta, want_m) in [(0x3eu32, 1u32, "ds_append"), (0x3d, 0xffff_ffff, "ds_consume")] {
        let mut g0 = init();
        g0[s] = if delta == 1 { u32::MAX - 3 } else { 3 };
        let isa: Vec<u8> = gds_op(op, 0, 0, 0, 0, 10).iter().flat_map(|w| w.to_le_bytes()).collect();
        let name = tombox_xshader::rdna2::disassemble(&isa)[0].mnemonic.clone();
        assert_eq!(name, want_m);
        for (label, threads, sel, on) in counter_cases() {
            let mut body = vec![vop1(1, 10, LIT), SENT, sop1(3, 40, 126)];
            body.extend_from_slice(&sel);
            body.extend_from_slice(&gds_op(op, 0, 0, 0, 0, 10));
            body.push(sop1(3, 126, 40));
            let words = prog(m0(s as u32 * 4, 4), &body, &[10]);
            let (mem, gds) = exec_gds_threads(&words, &g0, threads).unwrap_or_else(|e| panic!("{name} {label}: {e}"));
            let waves = threads.div_ceil(32);
            let lanes = |w: u32| 32 * w..(32 * w + 32).min(threads);
            let n: Vec<u32> = (0..waves).map(|w| lanes(w).filter(|&t| on(t)).count() as u32).collect();
            let total: u32 = n.iter().sum();
            assert_eq!(gds[s], g0[s].wrapping_add(delta.wrapping_mul(total)), "{name} {label}: counter");
            let mut o: Vec<usize> = gds.iter().enumerate().filter(|&(k, &v)| k != s && v != g0[k]).map(|(k, _)| k).collect();
            o.truncate(4);
            assert!(o.is_empty(), "{name} {label}: other GDS dwords changed {o:?}");
            for t in 0..threads {
                if !on(t) {
                    assert_eq!(mem[t as usize], SENT, "{name} {label}: inactive lane {t} was written");
                }
            }
            let bases: Vec<Option<u32>> = (0..waves).map(|w| lanes(w).find(|&t| on(t)).map(|t| mem[t as usize])).collect();
            for w in 0..waves {
                if let Some(b) = bases[w as usize] {
                    for t in lanes(w).filter(|&t| on(t)) {
                        assert_eq!(mem[t as usize], b, "{name} {label}: lane {t} of wave {w} differs from the wave's pre-op value");
                    }
                }
            }
            let live: Vec<(u32, u32)> = bases.iter().zip(&n).filter_map(|(b, &k)| b.map(|b| (b, k))).collect();
            let orders = if live.len() == 2 { vec![vec![0usize, 1], vec![1, 0]] } else { vec![(0..live.len()).collect()] };
            let fits = orders.iter().any(|ord| {
                let mut c = g0[s];
                ord.iter().all(|&i| {
                    let good = live[i].0 == c;
                    c = c.wrapping_add(delta.wrapping_mul(live[i].1));
                    good
                })
            });
            assert!(fits, "{name} {label}: wave pre-op values {live:08x?} are not a serial order of per-wave count_bits(exec) updates from {:08x}", g0[s]);
        }
        let words = prog(m0(TB_GDS_TITLE_DWORDS * 4, 4), &gds_op(op, 0, 0, 0, 0, 10), &[10]);
        let (mem, gds) = exec_gds(&words, &g0).unwrap();
        assert!(mem[..T as usize].iter().all(|&v| v == 0));
        assert_eq!(gds, g0);

        let mut pair = gds_op(op, 0, 0, 0, 0, 10).to_vec();
        pair.extend_from_slice(&gds_op(op, 0, 0, 0, 0, 11));
        let words = prog(m0(s as u32 * 4, 4), &pair, &[10, 11]);
        let (mem, gds) = exec_gds_threads(&words, &g0, 29).unwrap();
        let next = g0[s].wrapping_add(29u32.wrapping_mul(delta));
        assert!(mem[..29].iter().all(|&v| v == g0[s]));
        assert!(mem[T as usize..T as usize + 29].iter().all(|&v| v == next));
        assert_eq!(gds[s], next.wrapping_add(29u32.wrapping_mul(delta)));
    }
}

#[test]
fn ds_gds_counter_native_wave32() {
    // Group barriers inside a branch can deadlock. An LDS shader has already
    // allocated the full 32 KiB of groupshared memory.
    for mut body in [vec![vopc(0xd1, ic(20), 0), sopp(8, 2)], ds(0x0d, 0, 1, 2, 0, 0).to_vec()] {
        body.extend_from_slice(&gds_op(0x3e, 0, 0, 0, 0, 10));
        let words = prog(m0(BASE, 4), &body, &[10]);
        let t = translate(&cs_extract(&words, T)).unwrap();
        assert_eq!(t.profile, "cs_6_6");
        assert!(t.hlsl.contains("[WaveSize(32)]"));
        assert!(!t.hlsl.contains("groupshared uint tb_gds_mask"));
        dxc::compile(&t.hlsl, t.profile).unwrap();
    }
}
