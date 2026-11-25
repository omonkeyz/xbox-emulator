const TABLE: &str = include_str!("../exports.txt");
const LIB: &str = include_str!("lib.rs");

#[test]
fn export_table_matches_source() {
    let mut n = 0;
    for (i, line) in TABLE.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let head = line.split('|').next().unwrap();
        let f: Vec<&str> = head.split_whitespace().collect();
        assert_eq!(f[0].parse::<usize>().unwrap(), i + 1, "{line}");
        let name = f[1];
        let stub = LIB.contains(&format!("unimplemented_export!({name},"));
        let real = LIB.contains(&format!("fn {name}("));
        assert!(stub ^ real, "{name}");
        assert_eq!(f[2] == "stub", stub, "{name}");
        n += 1;
    }
    assert_eq!(n, 24);
    assert!(!TABLE.starts_with('\u{feff}'));
}
