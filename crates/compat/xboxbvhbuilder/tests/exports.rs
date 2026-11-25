use std::path::PathBuf;

fn built(file: &str) -> PathBuf {
    xone_exports::build_cdylib(env!("CARGO"), env!("CARGO_MANIFEST_DIR"), "xboxbvhbuilder", file)
}

#[test]
fn export_table_matches_exports_txt() {
    let rows = xone_exports::parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/exports.txt")).unwrap());
    let mut want: Vec<(u16, String)> = rows.iter().filter(|e| e.status != "missing").map(|e| (e.ordinal, e.name.clone())).collect();
    want.sort();
    let got = xone_exports::pe_exports(&std::fs::read(built("xboxbvhbuilder.dll")).unwrap()).unwrap();
    assert_eq!(got, want);
    assert!(!rows.iter().any(|e| e.status == "missing"));
}
