use super::passwordless::*;
use std::path::PathBuf;

const FN_LSASRV: &str = r"D:\xbox-pc-port\games\Fortnite\extracted\EmbeddedXvd\Windows\System32\lsasrv.dll";
const XD_ADVAPI32LEGACY: &str = r"D:\xbox-pc-port\games\XDefiant\extracted\EmbeddedXvd\Windows\System32\advapi32legacy.dll";

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("tombox_pwdless_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn rva_bytes(path: &str, rva: u32, n: usize) -> Option<Vec<u8>> {
    let f = std::fs::read(path).ok()?;
    let pe = u32::from_le_bytes(f[0x3c..0x40].try_into().unwrap()) as usize;
    let nsec = u16::from_le_bytes(f[pe + 6..pe + 8].try_into().unwrap()) as usize;
    let opt = u16::from_le_bytes(f[pe + 20..pe + 22].try_into().unwrap()) as usize;
    let sec = pe + 24 + opt;
    for i in 0..nsec {
        let s = sec + i * 40;
        let va = u32::from_le_bytes(f[s + 12..s + 16].try_into().unwrap());
        let vs = u32::from_le_bytes(f[s + 8..s + 12].try_into().unwrap());
        let raw = u32::from_le_bytes(f[s + 20..s + 24].try_into().unwrap());
        if rva >= va && rva < va + vs {
            let o = (raw + rva - va) as usize;
            return Some(f[o..o + n].to_vec());
        }
    }
    None
}

#[test]
fn opnum_policy_matches_console_lsasrv_table() {
    let Some(b) = rva_bytes(FN_LSASRV, 0x15e4f0 + 8 * 0x7e, 16) else { return };
    let e = |o: usize| OpnumPolicy { protseq_mask: u32::from_le_bytes(b[o..o + 4].try_into().unwrap()), flags: b[o + 4] };
    assert_eq!(e(0), LSARPC_POLICY_7E);
    assert_eq!(e(8), LSARPC_POLICY_7F);
    let auth = rva_bytes(FN_LSASRV, 0x15ed74, 8).unwrap();
    assert_eq!(&auth[..6], &[0, 0, 0, 0, 0, 5]);
}

#[test]
fn console_client_binding_matches_console_security_template() {
    let Some(b) = rva_bytes(XD_ADVAPI32LEGACY, 0x41b00, 0x18) else { return };
    let c = CallAttributes::console_client(OPNUM_ENABLE);
    assert_eq!(u32::from_le_bytes(b[0x10..0x14].try_into().unwrap()), c.authentication_level);
    assert_eq!(u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()), c.authentication_service);
    let name = rva_bytes(XD_ADVAPI32LEGACY, 0x4f960, 30).unwrap();
    let s: Vec<u16> = name.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16_lossy(&s), "LSARPC_ENDPOINT");
}

#[test]
fn security_callback_follows_console_gate() {
    let t = TitleToken::console_title();
    let ok = CallAttributes::console_client(OPNUM_ENABLE);
    assert_eq!(security_callback(&ok, &t), 0);
    assert_eq!(security_callback(&CallAttributes::console_client(OPNUM_DISABLE), &t), 0);
    assert_eq!(security_callback(&CallAttributes { protocol_sequence: 1, ..ok }, &t), ERROR_ACCESS_DENIED);
    assert_eq!(security_callback(&CallAttributes { is_client_local: 0, ..ok }, &t), ERROR_ACCESS_DENIED);
    for level in 2..=4 {
        assert_eq!(security_callback(&CallAttributes { authentication_level: level, ..ok }, &t), ERROR_ACCESS_DENIED);
    }
    assert_eq!(security_callback(&CallAttributes { authentication_level: 5, ..ok }, &t), 0);
    assert_eq!(security_callback(&CallAttributes { authentication_service: RPC_C_AUTHN_NETLOGON, ..ok }, &t), ERROR_ACCESS_DENIED);
    assert_eq!(security_callback(&CallAttributes { opnum: 0x81, ..ok }, &t), ERROR_ACCESS_DENIED);
    let app = TitleToken { is_app_container: true, ..t.clone() };
    assert_eq!(security_callback(&ok, &app), ERROR_ACCESS_DENIED);
    let outsider = TitleToken { user: "S-1-5-7".into(), groups: vec!["S-1-1-0".into()], is_app_container: false };
    assert_eq!(security_callback(&ok, &outsider), ERROR_ACCESS_DENIED);
}

#[test]
fn denied_call_maps_to_console_client_status() {
    let h = Hklm::at(temp("denied"));
    let app = TitleToken { is_app_container: true, ..TitleToken::console_title() };
    assert_eq!(lsarpc_call(&h, OPNUM_ENABLE, &CallAttributes::console_client(OPNUM_ENABLE), &app), 0xC000_0022u32 as i32);
    assert!(h.get_dword(DEVICE_KEY, DEVICE_VALUE).is_err());
    assert_eq!(client_exception_status(0), STATUS_UNSUCCESSFUL);
}

#[test]
fn lsa_enable_and_disable_password_less_write_the_device_value() {
    let d = temp("device");
    let h = Hklm::at(&d);
    let t = TitleToken::console_title();
    assert_eq!(h.get_dword(DEVICE_KEY, DEVICE_VALUE), Err(3));
    assert_eq!(lsarpc_call(&h, OPNUM_ENABLE, &CallAttributes::console_client(OPNUM_ENABLE), &t), 0);
    assert_eq!(h.get_dword(DEVICE_KEY, DEVICE_VALUE), Ok(2));
    assert!(h.value_path(DEVICE_KEY, DEVICE_VALUE).starts_with(&d));
    assert_eq!(lsarpc_call(&h, OPNUM_DISABLE, &CallAttributes::console_client(OPNUM_DISABLE), &t), 0);
    assert_eq!(h.get_dword(DEVICE_KEY, DEVICE_VALUE), Ok(0));
    assert!(h.get_dword(&format!("{USER_CONFIG_KEY}{}", t.user), USER_CONFIG_VALUE).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn force_disable_set_writes_the_user_config_value() {
    let d = temp("userconfig");
    let h = Hklm::at(&d);
    let t = TitleToken::console_title();
    h.create_key(IMAGE_CUSTOMIZATION_KEY).unwrap();
    h.set_dword(IMAGE_CUSTOMIZATION_KEY, FORCE_DISABLE_VALUE, 0).unwrap();
    assert!(force_disable_unset(&h));
    h.set_dword(IMAGE_CUSTOMIZATION_KEY, FORCE_DISABLE_VALUE, 1).unwrap();
    assert!(!force_disable_unset(&h));
    let key = format!("{USER_CONFIG_KEY}{}", t.user);
    assert_eq!(server_body(&h, true, &t), 0);
    assert_eq!(h.get_dword(&key, USER_CONFIG_VALUE), Ok(1));
    assert_eq!(server_body(&h, false, &t), 0);
    assert_eq!(h.get_dword(&key, USER_CONFIG_VALUE), Ok(2));
    assert!(h.get_dword(DEVICE_KEY, DEVICE_VALUE).is_err());
    h.set_dword(IMAGE_CUSTOMIZATION_KEY, FORCE_DISABLE_VALUE, 0).unwrap();
    assert_eq!(write_user_config(&h, &t.user, 1), E_NOTIMPL);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn failed_registry_write_maps_like_the_console_server() {
    let d = temp("fail");
    std::fs::create_dir_all(&d).unwrap();
    let blocker = d.join("SOFTWARE");
    std::fs::write(&blocker, b"x").unwrap();
    let h = Hklm::at(&d);
    let st = server_body(&h, true, &TitleToken::console_title());
    assert!(st < 0 && (st as u32) & 0xffff_0000 == 0xC007_0000, "{st:#x}");
    let _ = std::fs::remove_dir_all(&d);
}
