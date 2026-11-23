use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub fn target_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).parent().expect("target dir").to_path_buf()
}

pub fn profile_dir() -> PathBuf {
    let target = target_dir();
    let exe = std::env::current_exe().expect("test exe");
    exe.ancestors().find(|a| a.parent() == Some(target.as_path())).map(|a| a.to_path_buf()).unwrap_or_else(|| target.join("debug"))
}

fn cargo() -> PathBuf {
    std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"))
}

pub fn ensure(pkgs: &[(&str, &str)]) -> PathBuf {
    static BUILT: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = profile_dir();
    let todo: Vec<&str> = pkgs.iter().map(|p| p.0).filter(|p| !built.iter().any(|b| b == p)).collect();
    if !todo.is_empty() {
        let mut cmd = Command::new(cargo());
        cmd.arg("build").current_dir(env!("CARGO_MANIFEST_DIR"));
        for (k, _) in std::env::vars_os() {
            let key = k.to_string_lossy().to_ascii_uppercase();
            let dxc = key == "TOMBOX_DXC" || key == "TOMBOX_DXC_DIR";
            let injected = (key.starts_with("CARGO_") && key != "CARGO_HOME") || key == "OUT_DIR" || key.starts_with("RUST_TEST") || (key.starts_with("TOMBOX_") && !dxc);
            if injected {
                cmd.env_remove(&k);
            }
        }
        cmd.env("CARGO_TARGET_DIR", target_dir());
        if dir.file_name().and_then(|n| n.to_str()) == Some("release") {
            cmd.arg("--release");
        }
        for p in &todo {
            cmd.arg("-p").arg(p);
        }
        let out = cmd.output().unwrap_or_else(|e| panic!("running {:?}: {e}", cmd));
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
            let errors: Vec<&str> = lines.iter().enumerate().filter(|(_, l)| l.starts_with("error")).flat_map(|(i, _)| lines[i..(i + 6).min(lines.len())].iter().copied()).collect();
            let tail = &lines[lines.len().saturating_sub(15)..];
            panic!("cargo build -p {} into {} failed:\n{}\n...\n{}", todo.join(" -p "), target_dir().display(), errors.join("\n"), tail.join("\n"));
        }
        built.extend(todo.iter().map(|p| p.to_string()));
    }
    for (pkg, file) in pkgs {
        assert!(dir.join(file).is_file(), "{file} (package {pkg}) not in {} after cargo build", dir.display());
    }
    dir
}
