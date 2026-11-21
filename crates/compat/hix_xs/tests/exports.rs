use std::path::PathBuf;

fn built(file: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let mut dirs: Vec<PathBuf> = std::env::var_os("CARGO_TARGET_DIR").map(|d| vec![PathBuf::from(d).join("debug")]).unwrap_or_default();
    dirs.extend(exe.ancestors().skip(1).flat_map(|a| [a.to_path_buf(), a.join("debug")]));
    dirs.into_iter()
        .map(|d| d.join(file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} not built; run cargo build -p hix_xs before cargo test"))
}

#[test]
fn export_table_matches_exports_txt() {
    let rows = xone_exports::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/exports.txt")).unwrap());
    let mut want: Vec<(u16, String)> = rows.iter().filter(|e| e.status != "missing").map(|e| (e.ordinal, e.name.clone())).collect();
    want.sort();
    let got = xone_exports::pe_exports(&std::fs::read(built("hix_xs.dll")).unwrap()).unwrap();
    assert_eq!(got, want);
    for name in ["DebugLive", "WriteHangDump"] {
        assert!(rows.iter().any(|e| e.name == name && e.status == "missing"));
        assert!(!got.iter().any(|(_, n)| n == name));
    }
    assert!(got.iter().any(|(o, n)| *o == 3 && n == "CollectHangSummary"));
}
