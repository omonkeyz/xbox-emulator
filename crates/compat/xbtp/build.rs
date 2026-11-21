fn main() {
    println!("cargo:rerun-if-changed=exports.txt");
    let text = std::fs::read_to_string("exports.txt").expect("exports.txt");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").map(|e| e == "msvc").unwrap_or(false);
    if !msvc {
        return;
    }
    println!("cargo:rerun-if-changed=version.rc");
    let parts = [
        env!("CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"), "0",
    ];
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("xbtp_version.h"), format!(
        "#define XBTP_VERSION {}\n#define XBTP_VERSION_STRING \"{}\"\n",
        parts.join(","), parts.join("."),
    )).expect("write version header");
    embed_resource::compile_for_cdylib("version.rc", embed_resource::NONE)
        .manifest_required().expect("compile XBTP version resource");
    for e in xone_exports::parse(&text) {
        if e.status == "missing" {
            continue;
        }
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{},@{}", e.name, e.ordinal);
    }
}
