import struct, sys

WORDS = {
    "v_fmac_f16": [0x6C140501],
    "v_fmamk_f16": [0x6E140501, 0x00003555],
    "v_fmaak_f16": [0x70140501, 0x0000C248],
    "v_pk_lshlrev_b16": [0xCC04400A, 0x18020501],
    "v_pk_lshrrev_b16": [0xCC05400A, 0x18020501],
    "v_cvt_pknorm_i16_f32": [0xD768000A, 0x00020501],
    "v_cvt_pknorm_u16_f32": [0xD769000A, 0x00020501],
    "v_cvt_pknorm_i16_f16": [0xD712000A, 0x00020501],
    "v_cvt_pknorm_u16_f16": [0xD713000A, 0x00020501],
    "v_cvt_norm_i16_f16": [0x7E14C701],
    "v_cvt_norm_u16_f16": [0x7E14C901],
    "s_bfe_u32": [0x93981514, 0x851A8081],
    "s_bfe_i32": [0x94181514, 0x851A8081],
    "s_bfe_u64": [0x94981614, 0x851A8081],
    "s_bfe_i64": [0x95181614, 0x851A8081],
}


def rows(path, n):
    d = open(path, "rb").read()
    return [struct.unpack_from("<4I", d, 16 * i) for i in range(n)]


def main():
    d, out, n, source = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
    lines = ["use super::super::*;", "", f'const SOURCE: &str = "{source}";', ""]
    lines.append("const CASES: &[(&str, &[u32], [u32; 4], [u32; 4])] = &[")
    for name, words in WORDS.items():
        ins, outs = rows(f"{d}/in/{name}.in", n), rows(f"{d}/out/{name}.a", n)
        w = ", ".join(f"{x:#010x}" for x in words)
        for a, o in zip(ins, outs):
            lines.append(f'    ("{name}", &[{w}], [{", ".join(f"{x:#010x}" for x in a)}], [{", ".join(f"{x:#010x}" for x in o)}]),')
    lines.append("];")
    lines.append("")
    lines.append("#[test]")
    lines.append("""fn rdna2_silicon_results() {
    let mut c = Cases::new();
    let hw = std::env::var_os("TOMBOX_EXEC_HW").is_some();
    let mut nan_class = 0;
    for &(m, op, i, o) in CASES {
        let mut p = pr();
        if m.starts_with("s_") {
            p.sv(20, i[0]).sv(21, i[1]).sv(22, i[2]).sv(23, i[3]);
            let wide = m.ends_with("64");
            let mut post = pr();
            post.s2v(0, 24).s2v(1, 26);
            if wide {
                post.s2v(2, 25);
            }
            let exp: Vec<u32> = if wide { vec![o[1], o[3], o[2]] } else { vec![o[1], o[3]] };
            c.add(m, &p, op, &post, &exp);
        } else {
            p.vv(1, i[0]).vv(2, i[1]).vv(3, i[2]).vv(10, i[3]);
            let nan16 = o[0] & 0x7c00 == 0x7c00 && o[0] & 0x3ff != 0;
            if hw && nan16 && m.starts_with("v_fma") {
                let mut post = pr();
                post.raw(&[vop1(1, 11, ic(3)), vopc(0x8f, v(10), 11)]).vcc_to_v(0).raw(&[vop2(0x16, 1, ic(16), 10)]);
                c.add(m, &p, op, &post, &[1, o[0] >> 16]);
                nan_class += 1;
                continue;
            }
            let mut post = pr();
            post.raw(&[vop1(1, 0, v(10))]);
            c.add(m, &p, op, &post, &[o[0]]);
        }
    }
    if hw {
        println!("TOMBOX_EXEC_HW: {nan_class} f16 FMA cases whose recorded result is a NaN compare as a NaN class (the host GPU's NaN payload need not be RDNA2's); WARP compares them bit-exact");
    }
    println!("expected values recorded on {SOURCE}");
    c.run_both("rdna2_silicon");
}""")
    open(out, "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
