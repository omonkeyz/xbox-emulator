use std::path::PathBuf;
use tombox_xshader::{extract, hlsl, meta};

fn psv0_body(entries: &[[u32; 6]]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    b.extend_from_slice(&24u32.to_le_bytes());
    for e in entries {
        for w in e {
            b.extend_from_slice(&w.to_le_bytes());
        }
    }
    b
}

#[test]
fn psv0_resource_kind_is_read_from_the_v1_bind_records() {
    let r = meta::parse_psv0(&psv0_body(&[[3, 0, 110, 110, 4, 0], [3, 0, 85, 85, 2, 0]])).unwrap();
    assert_eq!((r[0].lower, r[0].kind), (110, 4));
    assert_eq!((r[1].lower, r[1].kind), (85, 2));
}

#[test]
fn gcn_image_dim_follows_the_declared_resource_kind() {
    assert_eq!(hlsl::psv_image_dim(4, 1), 2);
    assert_eq!(hlsl::psv_image_dim(5, 1), 3);
    assert_eq!(hlsl::psv_image_dim(1, 1), 0);
    assert_eq!(hlsl::psv_image_dim(6, 5), 4);
    assert_eq!(hlsl::psv_image_dim(2, 1), 1);
    assert_eq!(hlsl::psv_image_dim(7, 5), 5);
    assert_eq!(hlsl::psv_image_dim(4, 5), 5);
    assert_eq!(hlsl::psv_image_dim(0, 1), 1);
}

#[test]
fn bindless_light_grid_of_a_durango_pixel_shader_is_loaded_as_a_3d_texture() {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../input/shader_cache/titles/GbxGame/src/f861c56a34758d84_8200000000000000.xbox");
    let Ok(blob) = std::fs::read(&p) else {
        eprintln!("skipped: {} not present", p.display());
        return;
    };
    let ex = extract(&blob).unwrap();
    let t = hlsl::translate(&ex).unwrap();
    assert!(t.hlsl.contains("Texture3D<uint4> tb_bl"), "light grid not declared 3D");
    assert!(t.hlsl.contains(".Load(int4((int)v44, (int)v45, (int)v46, 0))"), "light grid load lost its slice coordinate");
}
