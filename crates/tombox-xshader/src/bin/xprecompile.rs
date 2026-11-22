use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tombox_xshader::uearchive::{find_archives, lz4_block};
use tombox_xshader::{dxc, extract, hlsl, runtime_hlsl, Stage};

fn fnv1a(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn stage_name(s: &Stage) -> &'static str {
    match s {
        Stage::Vertex => "VS",
        Stage::Pixel => "PS",
        Stage::Compute => "CS",
        Stage::Geometry => "GS",
        Stage::Hull => "HS",
        Stage::Domain => "DS",
        Stage::Mesh => "MS",
        Stage::Amplification => "AS",
        Stage::Unknown(_) => "??",
    }
}

fn parse_src_name(name: &str) -> Option<(u64, [u8; 8])> {
    let stem = name.strip_suffix(".xbox")?;
    let (h, f) = stem.split_once('_')?;
    if h.len() != 16 || f.len() != 16 {
        return None;
    }
    let hash = u64::from_str_radix(h, 16).ok()?;
    let mut rt = [0u8; 8];
    for i in 0..8 {
        rt[i] = u8::from_str_radix(&f[2 * i..2 * i + 2], 16).ok()?;
    }
    Some((hash, rt))
}

fn containers(data: &[u8], seen: &mut HashSet<u64>, out: &mut Vec<(Vec<u8>, [u8; 8])>) -> usize {
    let mut found = 0;
    let mut p = 0usize;
    while p + 32 <= data.len() {
        if &data[p..p + 4] != b"DXBC" {
            p += 1;
            continue;
        }
        let total = u32::from_le_bytes(data[p + 24..p + 28].try_into().unwrap()) as usize;
        let nchunks = u32::from_le_bytes(data[p + 28..p + 32].try_into().unwrap()) as usize;
        if total < 64 || total > 0x40_0000 || p + total > data.len() || nchunks == 0 || nchunks > 32 {
            p += 4;
            continue;
        }
        let blob = &data[p..p + total];
        p += total;
        found += 1;
        if seen.insert(fnv1a(blob)) {
            out.push((blob.to_vec(), [0; 8]));
        }
    }
    found
}

fn default_src_dir() -> std::path::PathBuf {
    tombox_xshader::shader_cache::src_dir("FortniteClient-XSX-Shipping")
}

#[derive(Default)]
struct Tally {
    by_stage: BTreeMap<String, [usize; 5]>,
    fails: BTreeMap<String, usize>,
    slow: Vec<(u128, u64, String)>,
    compile_ms: u128,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut inputs: Vec<String> = Vec::new();
    let mut src_dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(4).max(1);
    let mut limit = usize::MAX;
    let mut dry = false;
    let mut bench: Option<(String, String)> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--src" => {
                src_dirs.push(args.get(i + 1).map(std::path::PathBuf::from).unwrap_or_else(default_src_dir));
                i += 2;
            }
            "--known" => {
                src_dirs.push(default_src_dir());
                src_dirs.push(std::env::temp_dir().join("tombox_shader_cache").join("src").join("FortniteClient-XSX-Shipping"));
                src_dirs.push(std::env::temp_dir().join("tombox_shader_cache").join("pack").join("titles").join("FortniteClient-XSX-Shipping").join("src"));
                i += 1;
            }
            "--jobs" => {
                jobs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(jobs).max(1);
                i += 2;
            }
            "--limit" => {
                limit = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(limit);
                i += 2;
            }
            "--dry" => {
                dry = true;
                i += 1;
            }
            "--bench-replace" => {
                let pick = |k: usize| args.get(i + k).and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim_end().to_string());
                match (pick(1), pick(2)) {
                    (Some(from), Some(to)) => bench = Some((from, to)),
                    _ => {
                        eprintln!("--bench-replace needs two readable files");
                        return ExitCode::FAILURE;
                    }
                }
                i += 3;
            }
            _ => {
                inputs.push(args[i].clone());
                i += 1;
            }
        }
    }
    if inputs.is_empty() && src_dirs.is_empty() {
        eprintln!("usage: xprecompile [--known | --src <dir>] [<shader archive or .pak>...] [--jobs n] [--limit n] [--dry]");
        eprintln!("fills the runtime DXIL pack ({}) with exactly the HLSL the runtime would compile; inputs are only read", tombox_xshader::shader_cache::pack_path().display());
        return ExitCode::FAILURE;
    }
    if !dxc::available() {
        eprintln!("dxcompiler.dll + dxil.dll not found");
        return ExitCode::FAILURE;
    }
    unsafe {
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn SetPriorityClass(h: *mut std::ffi::c_void, class: u32) -> i32;
        }
        SetPriorityClass(GetCurrentProcess(), 0x4000);
    }
    let probe = tombox_xshader::fma_probe::probe();
    let fma = probe.fma;
    eprintln!("pack {} ({} records); fma probe on {}: preserve {} any {} [{}]{}", tombox_xshader::shader_cache::pack_path().display(), tombox_xshader::shader_cache::stats().records, probe.adapter, fma.preserve, fma.any, probe.detail, if probe.cached { " (cached)" } else { "" });
    let t_gather = std::time::Instant::now();
    let mut work: Vec<(Vec<u8>, [u8; 8])> = Vec::new();
    let mut seen_pairs: HashSet<(u64, [u8; 8])> = HashSet::new();
    for d in &src_dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            eprintln!("cannot read {}", d.display());
            continue;
        };
        let mut n = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some((h, rt)) = parse_src_name(&name) else { continue };
            let Ok(blob) = std::fs::read(e.path()) else { continue };
            if fnv1a(&blob) != h || !seen_pairs.insert((h, rt)) {
                continue;
            }
            work.push((blob, rt));
            n += 1;
        }
        eprintln!("{}: {n} known (shader, render-target format) pairs", d.display());
    }
    let mut seen: HashSet<u64> = work.iter().filter(|(_, rt)| *rt == [0; 8]).map(|(b, _)| fnv1a(b)).collect();
    for f in &inputs {
        let mut input = match std::fs::File::open(f).map(BufReader::new) {
            Ok(reader) => reader,
            Err(e) => {
                eprintln!("cannot open {f}: {e}");
                continue;
            }
        };
        if input.fill_buf().is_ok_and(|header| header.starts_with(b"9FPR")) {
            let before = work.len();
            let result = tombox_xshader::rpf::scan(input, |record| {
                if seen.insert(fnv1a(record.dxbc)) {
                    work.push((record.dxbc.to_vec(), [0; 8]));
                }
                Ok(())
            });
            if let Err(e) = result {
                eprintln!("cannot scan RPF9 archive {f}: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("{f}: RPF9, {} new unique shaders", work.len() - before);
            continue;
        }
        let mut data = Vec::new();
        if let Err(e) = input.read_to_end(&mut data) {
            eprintln!("cannot read {f}: {e}");
            continue;
        }
        let archives = find_archives(&data);
        let before = work.len();
        let mut found = 0;
        if archives.is_empty() {
            found += containers(&data, &mut seen, &mut work);
        } else {
            let mut lz_fail = 0;
            for a in &archives {
                for &(off, size, usize_) in &a.entries {
                    let src = &data[off..off + size];
                    match if size == usize_ { Some(src.to_vec()) } else { lz4_block(src, usize_) } {
                        Some(d) => found += containers(&d, &mut seen, &mut work),
                        None => lz_fail += 1,
                    }
                }
            }
            if lz_fail > 0 {
                eprintln!("{f}: {lz_fail} LZ4 entries failed to decompress");
            }
        }
        eprintln!("{f}: {} archives, {found} containers, {} new unique", archives.len(), work.len() - before);
    }
    work.truncate(limit);
    eprintln!("{} shaders to check, gathered in {:.1}s, {jobs} jobs", work.len(), t_gather.elapsed().as_secs_f64());
    let loop_grad = runtime_hlsl::loop_grad_from_env();
    let tally = Mutex::new(Tally::default());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let started = std::time::Instant::now();
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((blob, rt)) = work.get(i) else { break };
                let h = fnv1a(blob);
                let r = std::panic::catch_unwind(|| {
                    let ex = extract(blob).map_err(|e| ("?".to_string(), format!("extract: {e}")))?;
                    let st = stage_name(&ex.stage).to_string();
                    let t = hlsl::translate(&ex).map_err(|e| (st.clone(), format!("translate: {}", e.lines().next().unwrap_or(""))))?;
                    let src = runtime_hlsl::finalize(&t.hlsl, rt, loop_grad);
                    let profile = dxc::profile_for(t.profile);
                    if let Some((from, to)) = &bench {
                        let src = src.replacen(from.as_str(), to, 1);
                        let t0 = std::time::Instant::now();
                        dxc::compile(&src, &profile).map_err(|e| (st.clone(), e))?;
                        return Ok((st, t0.elapsed().as_millis(), true, src.lines().count()));
                    }
                    if tombox_xshader::shader_cache::contains(tombox_xshader::shader_cache::key(&src, &profile, fma, "")) {
                        return Ok((st, 0u128, false, src.lines().count()));
                    }
                    if dry {
                        return Ok((st, 0u128, true, src.lines().count()));
                    }
                    let c = tombox_xshader::shader_cache::compile_cached(&src, &profile, fma, &[]).map_err(|e| (st.clone(), e))?;
                    Ok((st, c.dxc_ms as u128, c.source == tombox_xshader::shader_cache::Source::Compiled, src.lines().count()))
                });
                let mut g = tally.lock().unwrap();
                match r {
                    Ok(Ok((st, ms, compiled, lines))) => {
                        let e = g.by_stage.entry(st).or_default();
                        e[0] += 1;
                        if compiled { e[1] += 1 } else { e[2] += 1 }
                        g.compile_ms += ms;
                        if ms > 3000 {
                            g.slow.push((ms, h, format!("{lines} lines rtfmt {:02x?}", rt)));
                        }
                    }
                    Ok(Err((st, e))) => {
                        let en = g.by_stage.entry(st).or_default();
                        en[0] += 1;
                        en[3] += 1;
                        let key: String = e.chars().take(90).collect();
                        *g.fails.entry(key).or_default() += 1;
                    }
                    Err(_) => {
                        g.by_stage.entry("??".into()).or_default()[4] += 1;
                    }
                }
                drop(g);
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 500 == 0 {
                    eprintln!("{n}/{} in {:.0}s", work.len(), started.elapsed().as_secs_f64());
                }
            });
        }
    });
    let g = tally.into_inner().unwrap();
    println!("stage  total  compiled  cache-hit  failed  panicked");
    for (st, c) in &g.by_stage {
        println!("{st:5} {:6} {:9} {:10} {:7} {:9}", c[0], c[1], c[2], c[3], c[4]);
    }
    println!("wall {:.1}s, summed DXC time {:.1}s", started.elapsed().as_secs_f64(), g.compile_ms as f64 / 1000.0);
    let mut slow = g.slow;
    slow.sort_by(|a, b| b.0.cmp(&a.0));
    for (ms, h, what) in slow.iter().take(15) {
        println!("slow {h:016x} {ms} ms ({what})");
    }
    for (e, n) in g.fails.iter().take(30) {
        println!("fail x{n}: {e}");
    }
    ExitCode::SUCCESS
}
