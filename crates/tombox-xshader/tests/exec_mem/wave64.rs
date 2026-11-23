use super::ds::cs_prog;
use super::*;
use tombox_xshader::pack_key::{self, CbvRaw, Mode};

const COUNTER: u32 = 768;
const ITERS: u32 = 64;
const WAIT_VM: u32 = 0xbf8c_3f70;
const WAIT_LGKM: u32 = 0xbf8c_c07f;

fn atomic_add_ret(vdata: u32, offset: u32) -> [u32; 2] {
    [0xe000_0000 | (0x32 << 18) | (1 << 14) | offset, (vdata << 8) | (2 << 16) | (0x80 << 24)]
}

fn handoff_body() -> Vec<u32> {
    handoff(false)
}

fn handoff(barrier: bool) -> Vec<u32> {
    let mut b = vec![sop1(0x03, 20, ic(0)), vop1(1, 12, ic(0)), vop1(1, 13, ic(0))];
    let head = b.len();
    b.extend_from_slice(&vop3(0xc2, 22, [ic(0), v(0), 0]));
    b.push(sop1(0x24, 24, 22));
    b.push(vop1(1, 14, ic(1)));
    b.extend_from_slice(&atomic_add_ret(14, COUNTER));
    b.push(WAIT_VM);
    b.push(vop1(1, 15, ic(0)));
    b.extend_from_slice(&ds::ds(0x0d, 0x28, 15, 14, 0, 0));
    if barrier {
        b.push(sopp(0x0a, 0));
    }
    b.push(sop2(0x11, 126, 126, 24));
    b.push(WAIT_LGKM);
    b.push(vop1(1, 16, ic(0)));
    b.extend_from_slice(&ds::ds(0x36, 0x28, 16, 0, 0, 17));
    b.push(WAIT_LGKM);
    b.push(vop2(0x25, 12, v(12), 17));
    b.push(vop2(0x1d, 18, 20, 17));
    b.push(vop2(0x1c, 13, v(13), 18));
    b.push(sop2(0x00, 20, 20, ic(1)));
    b.push(sopc(0x0a, 20, ic(ITERS as i32)));
    let back = head as i32 - (b.len() as i32 + 1);
    b.push(sopp(0x05, back as i16 as u16));
    b
}

fn lanes_body() -> Vec<u32> {
    let mut b = vop3(0x360, 30, [v(2), ic(40), 0]).to_vec();
    b.push(vop1(1, 19, 30));
    b.extend_from_slice(&[sop1(0x03, 31, LIT), 0x1234_5678]);
    b.push(vop1(1, 20, v(2)));
    b.extend_from_slice(&vop3(0x361, 20, [31, ic(45), 0]));
    b.extend_from_slice(&[sop1(0x24, 24, ic(-1))]);
    b
}

fn extract(words: &[u32]) -> tombox_xshader::ShaderExtract {
    let mut ex = cs_extract(words, 64);
    ex.resources = vec![tombox_xshader::meta::PsvResource { res_type: 7, space: 0, lower: 0, upper: 7, kind: 1 }];
    ex
}

fn mode(wave64: bool) -> Mode {
    Mode { cbv_raw: CbvRaw::Off, loop_grad: true, fma: tombox_xshader::shader_cache::Fma::default(), wave64, wave64_path: true }
}

fn run(words: &[u32], wave64: bool) -> Result<Vec<u32>, String> {
    let t = translate(&extract(words))?;
    let (src, profile) = pack_key::host_wave(&t.hlsl, &dxc::profile_for(t.profile), t.wave64, &mode(wave64));
    let code = dxc::compile(&src, &profile).map_err(|e| format!("dxc: {e}\n{src}"))?;
    let m = CTX.get_or_init(|| Mutex::new(unsafe { ctx_new() }));
    let mut c = m.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { exec_cs_in(&mut c, &src, &code, &[]) }.map_err(|e| format!("{e}\n{src}"))
}

fn hw_wave64() -> bool {
    if std::env::var_os("TOMBOX_EXEC_HW").is_none() {
        println!("skipped: needs TOMBOX_EXEC_HW=1 (WARP has no 64-lane waves)");
        return false;
    }
    let p = tombox_xshader::wave_probe::probe();
    println!("wave probe: {} [{}]", p.wave64, p.detail);
    if !p.wave64 {
        println!("skipped: the hardware adapter cannot run [WaveSize(64)]");
    }
    p.wave64
}

#[test]
fn wave64_compute_is_marked_and_gets_wavesize_only_on_a_wave64_host() {
    let t = translate(&extract(&cs_prog(&handoff_body(), &[12, 13, 17]))).unwrap();
    assert!(t.wave64, "s_and_saveexec_b64 makes this a wave64 program");
    let (src, profile) = pack_key::host_wave(&t.hlsl, &dxc::profile_for(t.profile), t.wave64, &mode(true));
    assert!(src.contains("[WaveSize(64)]\n[numthreads(64, 1, 1)]"), "{src}");
    assert_eq!(profile, "cs_6_6");
    let (narrow, _) = pack_key::host_wave(&t.hlsl, &dxc::profile_for(t.profile), t.wave64, &mode(false));
    assert!(!narrow.contains("WaveSize"));
    let w32 = translate(&cs_extract(&cs_prog(&[], &[2]), 64)).unwrap();
    assert!(!w32.wave64);
    assert!(!pack_key::host_wave(&w32.hlsl, "cs_6_2", w32.wave64, &mode(true)).0.contains("WaveSize"));
    let lanes = translate(&cs_extract(&cs_prog(&lanes_body(), &[19, 20]), 64)).unwrap();
    assert!(lanes.hlsl.contains("& (WaveGetLaneCount() - 1u)"), "{}", lanes.hlsl);
    if dxc::available() {
        dxc::compile(&src, &profile).expect("dxc compiles the wave64 program at cs_6_6");
    }
}

fn store_loop() -> Vec<u32> {
    let mut b = vec![sop1(0x03, 20, ic(0)), vop1(1, 15, ic(0)), vop1(1, 17, ic(3))];
    let head = b.len();
    b.extend_from_slice(&ds::ds(0x0d, 0x28, 15, 0, 0, 0));
    b.push(sop2(0x00, 20, 20, ic(1)));
    b.push(sopc(0x0a, 20, ic(4)));
    let back = head as i32 - (b.len() as i32 + 1);
    b.push(sopp(0x05, back as i16 as u16));
    b
}

fn straight() -> Vec<u32> {
    let mut b = vec![vop1(1, 15, ic(0))];
    b.extend_from_slice(&ds::ds(0x0d, 0x28, 15, 0, 0, 0));
    b.push(WAIT_LGKM);
    b.extend_from_slice(&ds::ds(0x36, 0x28, 15, 0, 0, 17));
    b.push(WAIT_LGKM);
    b
}

fn fences(body: &[u32]) -> (usize, String) {
    let t = translate(&extract(&cs_prog(body, &[17]))).unwrap();
    (t.hlsl.matches("GroupMemoryBarrier();").count(), t.hlsl)
}

#[test]
fn lds_fence_only_where_a_read_follows_a_nested_store_without_s_barrier() {
    let (n, src) = fences(&handoff(false));
    assert_eq!(n, 1, "predicated ds_write read back in the same loop: {src}");
    let (n, src) = fences(&handoff(true));
    assert_eq!(n, 0, "s_barrier between the ds_write and every ds_read: {src}");
    let (n, src) = fences(&store_loop());
    assert_eq!(n, 0, "no LDS read after the store: {src}");
    let (n, src) = fences(&straight());
    assert_eq!(n, 0, "top-level store and read: {src}");
    assert!(src.contains("GroupMemoryBarrierWithGroupSync();"), "{src}");
}

#[test]
fn wave64_host_switch_off_restores_the_previous_fences_lanes_and_wave_size() {
    let t = translate(&extract(&cs_prog(&handoff_body(), &[12, 13, 17]))).unwrap();
    let on = mode(true);
    let off = Mode { wave64: false, wave64_path: false, ..on };
    let f_on = pack_key::finalize(&t.hlsl, &[0; 8], &on);
    let f_off = pack_key::finalize(&t.hlsl, &[0; 8], &off);
    assert_eq!(f_on.matches("GroupMemoryBarrier();").count(), 1, "{f_on}");
    assert!(!f_off.contains("GroupMemoryBarrier();"), "{f_off}");
    assert_eq!(f_off.matches("GroupMemoryBarrierWithGroupSync").count(), f_on.matches("GroupMemoryBarrierWithGroupSync").count());
    let (s, p) = pack_key::host_wave(&f_off, &dxc::profile_for(t.profile), t.wave64, &off);
    assert!(!s.contains("WaveSize"), "{s}");
    let lanes = translate(&cs_extract(&cs_prog(&lanes_body(), &[19, 20]), 64)).unwrap();
    let l_off = pack_key::finalize(&lanes.hlsl, &[0; 8], &off);
    assert!(!l_off.contains("WaveGetLaneCount() - 1u") && l_off.contains("& 31u)"), "{l_off}");
    assert_ne!(pack_key::key(&f_on, &p, &on, &[]), pack_key::key(&s, &p, &off, &[]));
    if dxc::available() {
        dxc::compile(&s, &p).expect("dxc compiles the switched-off program");
    }
}

#[test]
fn hw_wave64_lds_handoff_without_barriers() {
    if !hw_wave64() {
        return;
    }
    let words = cs_prog(&handoff_body(), &[12, 13, 17]);
    let sum = (0..ITERS).sum::<u32>();
    let check = |o: &[u32]| -> Vec<String> {
        let mut bad = Vec::new();
        for t in 0..64usize {
            let got = [o[t], o[64 + t], o[128 + t]];
            if got != [sum, 0, ITERS - 1] {
                bad.push(format!("thread {t}: sum {} mismatch bits {:#x} last {}", got[0], got[1], got[2]));
            }
        }
        if o[COUNTER as usize / 4] != ITERS {
            bad.push(format!("counter {} want {ITERS}", o[COUNTER as usize / 4]));
        }
        bad
    };
    let narrow = run(&words, false).unwrap();
    let nb = check(&narrow);
    println!("driver default wave size: {} of 64 threads or counters wrong{}", nb.len(), nb.first().map(|s| format!(" (first: {s})")).unwrap_or_default());
    let wide = run(&words, true).unwrap();
    let wb = check(&wide);
    assert!(wb.is_empty(), "[WaveSize(64)]: {}", wb.join("; "));
}

#[test]
fn hw_wave64_readlane_and_writelane_reach_the_upper_half() {
    if !hw_wave64() {
        return;
    }
    let words = cs_prog(&lanes_body(), &[19, 20]);
    let o = run(&words, true).unwrap();
    let mut bad = Vec::new();
    for t in 0..64u32 {
        let want = [ds::dv(40), if t == 45 { 0x1234_5678 } else { ds::dv(t) }];
        let got = [o[t as usize], o[64 + t as usize]];
        if got != want {
            bad.push(format!("thread {t}: {:08x?} want {:08x?}", got, want));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("; "));
}
