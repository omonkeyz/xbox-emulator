#[cfg(not(windows))]
fn main() {
    eprintln!("fma_bench needs Windows");
}

#[cfg(windows)]
fn main() {
    use std::time::Instant;
    use tombox_xshader::{dxc, extract, hlsl};
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: fma_bench <shader.dxbc|.xbox>... [--variant file.hlsl] [--save dir]");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    let mut variant: Option<String> = None;
    let mut save: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--variant" => {
                variant = Some(std::fs::read_to_string(&args[i + 1]).expect("variant file"));
                i += 2;
            }
            "--save" => {
                save = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                files.push(args[i].clone());
                i += 1;
            }
        }
    }
    let mut total = 0u128;
    for f in &files {
        let blob = std::fs::read(f).expect("read");
        let ex = match extract(&blob) {
            Ok(e) => e,
            Err(e) => {
                println!("{f}: extract: {e}");
                continue;
            }
        };
        let t = match hlsl::translate(&ex) {
            Ok(t) => t,
            Err(e) => {
                println!("{f}: translate: {e}");
                continue;
            }
        };
        let mut src = t.hlsl.clone();
        let uses = src.matches("tb_fma(").count();
        if let Some(v) = &variant {
            let start = src.find("float tb_fma(float a, float b, float c) { precise double").expect("tb_fma def");
            let end = start + src[start..].find('\n').unwrap();
            src.replace_range(start..end, v.trim_end());
        }
        let profile = dxc::profile_for(t.profile);
        let t0 = Instant::now();
        let r = dxc::compile(&src, &profile);
        let ms = t0.elapsed().as_millis();
        total += ms;
        let stem = std::path::Path::new(f).file_stem().unwrap().to_string_lossy().into_owned();
        match &r {
            Ok(code) => println!("{stem}: {profile} {} lines, tb_fma uses {uses}, {ms} ms, {} bytes", src.lines().count(), code.len()),
            Err(e) => println!("{stem}: {profile} FAIL {ms} ms: {e}"),
        }
        if let Some(d) = &save {
            let _ = std::fs::create_dir_all(d);
            let _ = std::fs::write(std::path::Path::new(d).join(format!("{stem}.hlsl")), &src);
            if let Ok(code) = &r {
                let _ = std::fs::write(std::path::Path::new(d).join(format!("{stem}.dxil")), code);
            }
        }
    }
    println!("total {total} ms over {} shaders", files.len());
}
