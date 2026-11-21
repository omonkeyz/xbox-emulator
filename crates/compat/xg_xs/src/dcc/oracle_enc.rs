use super::oracle_dec::{copy, defined, patterns, smear, states, DecF, Enc, Lay, Tally, P1S};
use super::{dec, enc, px, Dcc};
use crate::dcc_oracle::{configs, diff, holes, Con, Rng};
use crate::layout::E_INVALIDARG;
use crate::oracle::{oracle, Oracle, SERIAL};

#[test]
fn probe_enc_bijective() {
    let mut bad = std::collections::BTreeMap::new();
    let mut n = 0;
    for (f, sw, dim, p0, _) in configs() {
        let mut d = Dcc::new();
        if d.init(f, sw, dim, p0, 0).is_err() {
            continue;
        }
        n += 1;
        let mut e = Dcc::new();
        e.m.copy_from_slice(&d.m);
        if e.r32(4) == 16 {
            e.w32(4, 8);
        }
        for (a, b, dd) in [(0u32, 1u32, &d), (1, 2, &d), (2, 3, &d), (3, 4, &e)] {
            for k in 0..4u32 {
                let mut seen = [false; 64];
                for x in k * 64..k * 64 + 64 {
                    let c = px::get(dd, x, a).unwrap();
                    let y = px::set(dd, &c, b).unwrap();
                    if y >> 6 != k {
                        *bad.entry(format!("{}->{} crosses seg", a, b)).or_insert(0) += 1;
                    }
                    seen[(y & 63) as usize] = true;
                }
                if seen.iter().any(|&s| !s) {
                    *bad.entry(format!("{}->{} f={} bpe={} sw={} dim={}", a, b, f, d.r32(4), d.r32(0), d.r32(0x14))).or_insert(0) += 1;
                }
            }
        }
    }
    println!("{} configs", n);
    for (k, v) in &bad {
        println!("{} {}", k, v);
    }
}

#[derive(Clone, Copy)]
enum St {
    Lay(usize, u32),
    C1(usize, fn(&mut Dcc) -> Result<(), i32>),
    Sub,
}

fn stages() -> Vec<(&'static str, St)> {
    vec![
        ("relayout b0->b2 mode1 0x4ac90", St::Lay(0, 1)),
        ("clear detect 0x26680", St::C1(0x26680, enc::clear_detect)),
        ("channel pack 0x27330", St::C1(0x27330, enc::channel)),
        ("signfix 0x27d10", St::C1(0x27d10, dec::signfix)),
        ("relayout mode2 0x4ac90", St::Lay(2, 2)),
        ("color fwd 0x27e50", St::C1(0x27e50, enc::color)),
        ("relayout mode3 0x4ac90", St::Lay(2, 3)),
        ("delta 0x28720", St::C1(0x28720, enc::delta)),
        ("vrs 0x28df0", St::C1(0x28df0, enc::vrs)),
        ("choose 0x292c0", St::C1(0x292c0, enc::choose)),
        ("far 0x29e40", St::C1(0x29e40, enc::far)),
        ("far base 0x29740 x4 + save/restore", St::Sub),
        ("plan 0x2a4e0", St::C1(0x2a4e0, enc::plan)),
        ("pack 0x2a8e0", St::C1(0x2a8e0, enc::pack)),
    ]
}

fn console_stage(o: &Oracle, c: &Con, s: &Dcc, st: St) -> Dcc {
    c.push(s);
    match st {
        St::Lay(src, mode) => {
            let lay: Lay = unsafe { core::mem::transmute(o.h + 0x4ac90) };
            let dst = c.buf(2).as_mut_ptr();
            let sp = c.buf(src).as_ptr();
            unsafe { lay(c.p(), dst, sp, mode) };
            c.pull()
        }
        St::C1(rva, _) => {
            c.call1(rva);
            c.pull()
        }
        St::Sub => {
            for k in 0..4 {
                c.call2(0x29740, k);
            }
            let mut p = c.pull();
            let b = p.b[2];
            p.m[0x1180..0x1280].copy_from_slice(&b);
            let w: Vec<u8> = p.m[0x34c..0x38c].to_vec();
            p.m[0x1280..0x12c0].copy_from_slice(&w);
            p.b[2] = s.b[2];
            p.m[0x34c..0x38c].copy_from_slice(&s.m[0x34c..0x38c]);
            p
        }
    }
}

fn native_stage(s: &Dcc, st: St) -> Result<Dcc, i32> {
    let mut n = copy(s);
    match st {
        St::Lay(src, mode) => enc::relay(&mut n, 2, src, mode)?,
        St::C1(_, f) => f(&mut n)?,
        St::Sub => enc::subbed(&mut n)?,
    }
    Ok(n)
}

fn inputs(r: &mut Rng, bpe: usize) -> Vec<[u8; 256]> {
    let mut v = patterns(r, bpe);
    for _ in 0..3 {
        let mut b = [0u8; 256];
        let base = r.u() as u8;
        for (i, x) in b.iter_mut().enumerate() {
            *x = base.wrapping_add((r.below(5) as u8).wrapping_sub(2)).wrapping_add((i / 64) as u8 * 17);
        }
        v.push(b);
    }
    v
}

#[test]
fn oracle_dcc_enc_stages() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let hl = holes(&o);
    let st = states();
    let covered: usize = st.iter().map(|x| x.1).sum();
    let mut r = Rng(0x5eed_e4c0_9876);
    let mut t = Tally::new();
    let (mut blocks, mut aborted, mut faults) = (0u64, 0u64, 0u64);
    let mut driver_bad = 0u64;
    for &((f, sw, dim, p0, p1), _) in &st {
        let Some(c) = Con::new(&o, f, sw, dim, p0, p1) else { continue };
        let base = defined(c.pull());
        let bpe = base.r32(4);
        let mut s = copy(&base);
        for inp in inputs(&mut r, bpe as usize) {
            blocks += 1;
            let ctx = format!("f={} sw={} dim={} p0={} p1={:#x}", f, sw, dim, p0, p1);
            s.b[0] = inp;
            s.w64(0x13c0, 0);
            s.w32(0xf0, 0);
            let start = copy(&s);
            let mut ok = true;
            for (name, stg) in stages() {
                if let St::C1(0x27e50, _) = stg {
                    if enc::color_faults(&s) {
                        faults += 1;
                        assert_eq!(enc::color(&mut copy(&s)), Err(E_INVALIDARG));
                        ok = false;
                        break;
                    }
                }
                let n = match native_stage(&s, stg) {
                    Ok(n) => n,
                    Err(_) => {
                        t.err(name);
                        ok = false;
                        break;
                    }
                };
                let p = console_stage(&o, &c, &s, stg);
                t.hit(name, diff(&n, &p, &hl, true), &ctx);
                s = p;
            }
            if !ok {
                aborted += 1;
                s = copy(&base);
                continue;
            }
            let mut n = copy(&start);
            enc::run(&mut n).expect("native driver failed after all native stages succeeded");
            c.push(&start);
            c.call1(0x26220);
            let p = c.pull();
            let m = diff(&n, &p, &hl, true);
            if m.is_some() {
                driver_bad += 1;
            }
            t.hit("driver 0x26220", m, &ctx);
            s = p;
        }
    }
    t.print("dcc enc stage");
    println!("dcc enc stages: {} unique states covering {} configs, {} blocks, {} aborted ({} color #DE predicted), driver mismatches {}", st.len(), covered, blocks, aborted, faults, driver_bad);
    assert_eq!(t.total_bad(), 0);
}

pub(super) fn console_encode(o: &Oracle, src: &[u8], count: usize, f: u32, sw: u32, dim: u32, p0: u32, p1: u32, fill: u8, sm: u8) -> (i32, Vec<u8>, Vec<u8>, Vec<u32>) {
    let enc: Enc = unsafe { core::mem::transmute(o.f("XGEncodeDCC")) };
    let prm = [p0, p1, 0, 0];
    let mut out = vec![fill; 256 * count];
    let mut meta = vec![fill; count];
    let mut sz = vec![u32::from_ne_bytes([fill; 4]); count];
    let _ = smear(sm);
    let r = unsafe { enc(count as u32, src.as_ptr(), out.as_mut_ptr(), meta.as_mut_ptr(), sz.as_mut_ptr(), f, sw, dim, prm.as_ptr()) };
    (r, out, meta, sz)
}

#[test]
fn oracle_dcc_enc_e2e() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let cdec: DecF = unsafe { core::mem::transmute(o.f("XGDecodeDCC")) };
    let st = states();
    let mut r = Rng(0xe2e_e4c_31);
    let (mut calls, mut blocks, mut bad, mut undef, mut nerr, mut faults, mut rt, mut rtbad, mut lossy) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut extra: Vec<(u32, u32, u32, u32, u32)> = configs();
    extra.retain(|_| r.below(30) == 0);
    let mut cfgs: Vec<(u32, u32, u32, u32, u32)> = st.iter().map(|x| x.0).collect();
    for (f, sw, dim, p0, _) in extra {
        cfgs.push((f, sw, dim, p0, P1S[r.below(5) as usize]));
    }
    for (f, sw, dim, p0, p1) in cfgs {
        let mut d = Dcc::new();
        if d.init(f, sw, dim, p0, p1).is_err() {
            continue;
        }
        let bpe = d.r32(4);
        let pats = inputs(&mut r, bpe as usize);
        for count in [1usize, 3, 8] {
            let mut src = vec![0u8; 256 * count];
            for i in 0..count {
                let p = pats[r.below(pats.len() as u32) as usize];
                src[i * 256..i * 256 + 256].copy_from_slice(&p);
            }
            let mut nd = vec![0u8; 256 * count];
            let mut nm = vec![0u8; count];
            let mut ns = vec![0u32; count];
            let nr = crate::dcc::encode(count as u32, &src, &mut nd, &mut nm, Some(&mut ns), f, sw, dim, p0, p1);
            if nr != 0 {
                assert_eq!(nr, E_INVALIDARG);
                if enc::color_faults(&d) {
                    faults += 1;
                } else {
                    nerr += 1;
                    if nerr <= 5 {
                        println!("e2e native E_INVALIDARG f={} sw={} dim={} p0={} p1={:#x} count={}", f, sw, dim, p0, p1, count);
                    }
                }
                continue;
            }
            let a = console_encode(&o, &src, count, f, sw, dim, p0, p1, 0xaa, 0xa5);
            let b = console_encode(&o, &src, count, f, sw, dim, p0, p1, 0x55, 0x5a);
            calls += 2;
            blocks += count as u64;
            if a != b {
                undef += 1;
                if undef <= 5 {
                    let j = (0..a.1.len()).find(|&j| a.1[j] != b.1[j]);
                    println!("e2e console output depends on prior memory f={} sw={} dim={} p0={} p1={:#x} count={} first dst diff {:?} meta {:02x?}/{:02x?}", f, sw, dim, p0, p1, count, j, a.2, b.2);
                }
            }
            if a.0 != 0 || a.1 != nd || a.2 != nm || a.3 != ns {
                bad += 1;
                if bad <= 10 {
                    let j = (0..nd.len()).find(|&j| a.1[j] != nd[j]);
                    println!("e2e f={} sw={} dim={} p0={} p1={:#x} count={}: hr={:#x} meta n{:02x?} c{:02x?} sizes n{:?} c{:?} first dst diff {:?}", f, sw, dim, p0, p1, count, a.0, nm, a.2, ns, a.3, j.map(|j| (j, nd[j], a.1[j])));
                }
            }
            let prm = [p0, p1, 0, 0];
            let mut cd = vec![0u8; 256 * count];
            let cr = unsafe { cdec(count as u32, nd.as_ptr(), nm.as_ptr(), cd.as_mut_ptr(), f, sw, dim, prm.as_ptr()) };
            let mut dd = vec![0u8; 256 * count];
            let dr = crate::dcc::decode(count as u32, &nd, &nm, &mut dd, f, sw, dim, p0, p1);
            rt += 1;
            if dr == 0 && (cr != 0 || cd != dd) {
                rtbad += 1;
                if rtbad <= 5 {
                    println!("round trip f={} sw={} dim={} p0={} p1={:#x} count={}: native decode differs from console decode", f, sw, dim, p0, p1, count);
                }
            }
            if cr == 0 && cd != src {
                lossy += 1;
            }
        }
    }
    println!("dcc enc e2e: {} XGEncodeDCC calls, {} blocks, {} mismatches, {} console results depending on prior memory, {} native E_INVALIDARG (non-fault), {} color #DE predicted, {} round trips, {} native/console decode mismatches, {} lossy round trips", calls, blocks, bad, undef, nerr, faults, rt, rtbad, lossy);
    assert_eq!(bad, 0);
    assert_eq!(rtbad, 0);
}

fn reject_cause(d0: &Dcc, inp: &[u8; 256]) -> &'static str {
    let mut d = copy(d0);
    d.b[0] = *inp;
    d.w64(0x13c0, 0);
    d.w32(0xf0, 0);
    let mut t = enc::Tmp::new();
    if enc::relay_in(&mut d, 2, 0, 1, &mut t).is_err() {
        return "relayout mode0->1 hole";
    }
    if enc::clear_detect(&mut d).is_err() {
        return "clear detect body";
    }
    let _ = enc::channel(&mut d);
    let _ = dec::signfix(&mut d);
    if enc::relay_in(&mut d, 2, 2, 2, &mut t).is_err() {
        return "relayout mode1->2 hole";
    }
    if enc::color_faults(&d) {
        return "color #DE";
    }
    "later stage"
}

#[test]
fn oracle_dcc_enc_undefined_classes() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let st = states();
    let mut r = Rng(0x0dd_e4c_5eed);
    let mut seen: std::collections::BTreeMap<&'static str, (u64, u64)> = std::collections::BTreeMap::new();
    let mut same = Vec::new();
    let mut grp: std::collections::BTreeMap<String, (u64, u64, String)> = std::collections::BTreeMap::new();
    for &((f, sw, dim, p0, p1), _) in &st {
        let mut d = Dcc::new();
        if d.init(f, sw, dim, p0, p1).is_err() {
            continue;
        }
        let bpe = d.r32(4);
        for inp in inputs(&mut r, bpe as usize).into_iter().take(3) {
            let mut nd = [0u8; 256];
            let mut nm = [0u8; 1];
            if crate::dcc::encode(1, &inp, &mut nd, &mut nm, None, f, sw, dim, p0, p1) == 0 {
                continue;
            }
            let cause = reject_cause(&d, &inp);
            if cause == "color #DE" || enc::color_faults(&d) {
                continue;
            }
            let a = console_encode(&o, &inp, 1, f, sw, dim, p0, p1, 0xaa, 0xa5);
            let b0 = console_encode(&o, &inp, 1, f, sw, dim, p0, p1, 0x55, 0x5a);
            let z = console_encode(&o, &inp, 1, f, sw, dim, p0, p1, 0x00, 0x00);
            let b = if a == b0 { z } else { b0 };
            let e = seen.entry(cause).or_insert((0, 0));
            e.0 += 1;
            let mut hd = copy(&d);
            hd.w32(0xf0, if cause == "relayout mode0->1 hole" { 0 } else { 1 });
            let hm = if cause == "relayout mode0->1 hole" { 1 } else { 2 };
            let hs = enc::relay_holes(&hd, hm);
            let g = grp.entry(format!("{} bpe={} s0={} dim={} s14={} v48={} holes={}", cause, d.r32(4), d.r32(0), dim, d.r32(0x14), d.r32(0x48), hs.len())).or_insert((0u64, 0u64, String::new()));
            g.0 += 1;
            if a != b {
                g.1 += 1;
                if g.2.is_empty() {
                    let j: Vec<usize> = (0..256).filter(|&j| a.1[j] != b.1[j]).take(8).collect();
                    g.2 = format!("diff dst {:?} meta {:02x?}/{:02x?} holes {:?}", j, a.2, b.2, &hs[..hs.len().min(12)]);
                }
            } else if g.2.is_empty() {
                g.2 = format!("stable meta {:02x?} holes {:?}", a.2, &hs[..hs.len().min(12)]);
            }
            if a != b {
                e.1 += 1;
            } else if same.len() < 10 {
                same.push(format!("{} f={} sw={} dim={} p0={} p1={:#x}", cause, f, sw, dim, p0, p1));
            }
        }
    }
    for (c, (n, dif)) in &seen {
        println!("dcc enc undefined class {}: {} native rejections, {} console outputs changed with stack smear", c, n, dif);
    }
    for (k, (n, dif, ex)) in &grp {
        println!("grp {}: {} rejections, {} changed; {}", k, n, dif, ex);
    }
    for s in &same {
        println!("dcc enc rejection with stable console output: {}", s);
    }
    assert!(seen.values().all(|&(n, d)| n == d));
}

#[test]
fn oracle_dcc_enc_relayout_holes() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = oracle() else { return };
    let lay: Lay = unsafe { core::mem::transmute(o.h + 0x4ac90) };
    let st = states();
    let (mut cases, mut bad, mut holed) = (0u64, 0u64, 0u64);
    for &((f, sw, dim, p0, p1), _) in &st {
        let Some(c) = Con::new(&o, f, sw, dim, p0, p1) else { continue };
        let base = c.pull();
        for (from, to, src) in [(0u32, 1u32, 0usize), (1, 2, 2), (2, 3, 2)] {
            let mut s = copy(&base);
            for j in 0..256 {
                s.b[0][j] = j as u8;
                s.b[2][j] = j as u8;
            }
            s.w32(0xf0, from);
            let mut outs = Vec::new();
            for v in [0xa5u8, 0x5a] {
                c.push(&s);
                let dst = c.buf(2).as_mut_ptr();
                let sp = c.buf(src).as_ptr();
                let _ = smear(v);
                unsafe { lay(c.p(), dst, sp, to) };
                outs.push(c.pull().b[2]);
            }
            let con: Vec<usize> = (0..256).filter(|&j| outs[0][j] != outs[1][j]).collect();
            let nat = enc::relay_holes(&s, to);
            cases += 1;
            if !nat.is_empty() {
                holed += 1;
            }
            if con != nat {
                bad += 1;
                if bad <= 8 {
                    println!("relayout holes f={} sw={} dim={} {}->{}: native {} {:?} console {} {:?}", f, sw, dim, from, to, nat.len(), &nat[..nat.len().min(10)], con.len(), &con[..con.len().min(10)]);
                }
            }
        }
    }
    println!("dcc enc relayout holes: {} cases, {} with holes, {} hole-set mismatches", cases, holed, bad);
    assert_eq!(bad, 0);
}
