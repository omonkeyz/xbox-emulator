use std::cmp::Reverse;
use std::path::{Path, PathBuf};

/// Versioned Windows SDK directories containing the x64 host compiler, newest first.
pub fn sdk_dirs() -> Vec<PathBuf> {
    let bin = std::env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .map(|root| root.join("bin"));
    let default_bin = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join(r"Windows Kits\10\bin");
    let mut dirs = bin.as_deref().map(compilers_in).unwrap_or_default();
    if bin.as_deref() != Some(&default_bin) {
        dirs.extend(compilers_in(&default_bin));
    }
    dirs
}

fn compilers_in(bin: &Path) -> Vec<PathBuf> {
    let Ok(entries) = bin.read_dir() else {
        return Vec::new();
    };
    let mut compilers: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let version: Vec<u32> = name
                .to_str()?
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            let dir = entry.path().join("x64");
            dir.join("dxcompiler.dll")
                .is_file()
                .then_some((Reverse(version), dir))
        })
        .collect();
    compilers.sort();
    compilers.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_x64_compilers_in_numeric_version_order() {
        let root = std::env::temp_dir().join(format!("tombox-dxc-sdk-{}", std::process::id()));
        for (name, arch) in [
            ("10.0.9.0", "x64"),
            ("10.0.10.0", "x64"),
            ("10.0.11.0", "x86"),
            ("preview", "x64"),
        ] {
            let dir = root.join(name).join(arch);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("dxcompiler.dll"), []).unwrap();
        }
        assert_eq!(
            compilers_in(&root),
            [root.join("10.0.10.0/x64"), root.join("10.0.9.0/x64")]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
