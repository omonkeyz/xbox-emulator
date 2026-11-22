use std::process::Command;

fn assert_argument_error(args: &[&str], message: &str) {
    let result = Command::new(env!("CARGO_BIN_EXE_xshader"))
        .arg("unused.dxbc")
        .args(args)
        .output()
        .expect("run xshader");
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(message), "{stderr}");
}

#[test]
fn rejects_unknown_option_before_opening_shader() {
    assert_argument_error(
        &["--root-signatue", "root.bin", "--hlsl-out", "translated.hlsl"],
        "unknown option --root-signatue",
    );
}

#[test]
fn rejects_missing_file_argument_even_when_another_option_follows() {
    for args in [&["--root-signature"][..], &["--root-signature", "--hlsl-out", "translated.hlsl"]] {
        assert_argument_error(args, "missing file argument for --root-signature");
    }
}
