
use core::ffi::c_void;
use std::sync::OnceLock;

extern "system" {
    fn LocaleNameToLCID(name: *const u16, flags: u32) -> u32;
    fn LCIDToLocaleName(lcid: u32, name: *mut u16, cch: i32, flags: u32) -> i32;
    fn SetLastError(code: u32);
    fn RtlLookupFunctionEntry(pc: u64, image_base: *mut u64, history: *mut c_void) -> *const u32;
}

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_PARAMETER: u32 = 87;
const MUI_LANGUAGE_ID: u32 = 0x4;
const MUI_LANGUAGE_NAME: u32 = 0x8;
const LOCALE_CUSTOM_UNSPECIFIED: u32 = 0x1000;
const JUMP_LEN: u64 = 12;

#[derive(Debug, PartialEq, Eq)]
pub struct Locale {
    pub name: String,
    pub lcid: u32,
}

impl Locale {
    fn langid(&self) -> u16 {
        (self.lcid & 0xffff) as u16
    }
    fn id_string(&self) -> String {
        format!("{:04X}", self.langid())
    }
}

pub fn resolve(configured: &str) -> Result<Locale, String> {
    let wide: Vec<u16> = configured.trim().encode_utf16().chain(Some(0)).collect();
    let lcid = unsafe { LocaleNameToLCID(wide.as_ptr(), 0) };
    if lcid == 0 {
        return Err(format!("'{configured}' is not a locale name Windows knows"));
    }
    if lcid == LOCALE_CUSTOM_UNSPECIFIED {
        return Err(format!("'{configured}' has no LCID of its own, so LCID-based queries could not report it"));
    }
    let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
    let n = unsafe { LCIDToLocaleName(lcid, buf.as_mut_ptr(), buf.len() as i32, 0) };
    if n <= 1 {
        return Err(format!("LCID {lcid:#x} of '{configured}' has no name"));
    }
    Ok(Locale { name: String::from_utf16_lossy(&buf[..n as usize - 1]), lcid })
}

fn put(text: &str, terminators: usize, buf: *mut u16, cch: usize) -> (usize, bool) {
    let units: Vec<u16> = text.encode_utf16().chain(core::iter::repeat(0).take(terminators)).collect();
    if buf.is_null() || cch < units.len() {
        return (units.len(), false);
    }
    unsafe { core::ptr::copy_nonoverlapping(units.as_ptr(), buf, units.len()) };
    (units.len(), true)
}

static ACTIVE: OnceLock<Locale> = OnceLock::new();

fn active() -> &'static Locale {
    ACTIVE.get().expect("locale hooks are installed only after ACTIVE is set")
}

unsafe extern "system" fn get_user_default_locale_name(buf: *mut u16, cch: i32) -> i32 {
    unsafe { LCIDToLocaleName(active().lcid, buf, cch, 0) }
}

unsafe extern "system" fn get_user_default_lcid() -> u32 {
    active().lcid
}

unsafe extern "system" fn get_user_default_langid() -> u16 {
    active().langid()
}

unsafe extern "system" fn get_user_preferred_ui_languages(flags: u32, count: *mut u32, buf: *mut u16, cch: *mut u32) -> i32 {
    if cch.is_null() || flags & !(MUI_LANGUAGE_ID | MUI_LANGUAGE_NAME) != 0
        || flags & (MUI_LANGUAGE_ID | MUI_LANGUAGE_NAME) == (MUI_LANGUAGE_ID | MUI_LANGUAGE_NAME) {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let text = if flags & MUI_LANGUAGE_ID != 0 { active().id_string() } else { active().name.clone() };
    let (needed, fitted) = put(&text, 2, buf, *cch as usize);
    let size_query = buf.is_null() && *cch == 0;
    *cch = needed as u32;
    if !fitted && !size_query {
        SetLastError(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    if !count.is_null() {
        *count = 1;
    }
    1
}

unsafe fn patch_fits(p: *mut u8) -> bool {
    let mut base = 0u64;
    let f = RtlLookupFunctionEntry(p as u64, &mut base, core::ptr::null_mut());
    if !f.is_null() && base + u64::from(*f) == p as u64 {
        return u64::from(*f.add(1)) - u64::from(*f) >= JUMP_LEN;
    }
    let bytes = core::slice::from_raw_parts(p as *const u8, JUMP_LEN as usize);
    bytes[0] == 0xE9 && bytes[5..].iter().all(|&b| b == 0xCC)
}

pub fn install(configured: &str) {
    let loc = match resolve(configured) {
        Ok(l) => l,
        Err(e) => {
            crate::record_raw("locale", "config", &format!("locale={configured} ignored: {e}; the host language stays in effect"));
            return;
        }
    };
    let summary = format!("{} (LCID {:#06x})", loc.name, loc.lcid);
    let _ = ACTIVE.set(loc);
    let hooks: [(&[u8], usize); 5] = [
        (b"GetUserDefaultLocaleName\0", get_user_default_locale_name as *const () as usize),
        (b"GetUserDefaultLCID\0", get_user_default_lcid as *const () as usize),
        (b"GetUserDefaultLangID\0", get_user_default_langid as *const () as usize),
        (b"GetUserDefaultUILanguage\0", get_user_default_langid as *const () as usize),
        (b"GetUserPreferredUILanguages\0", get_user_preferred_ui_languages as *const () as usize),
    ];
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for (name, hook) in hooks {
        let label = String::from_utf8_lossy(&name[..name.len() - 1]).into_owned();
        let p = unsafe { crate::net_guard::resolve_export(b"kernelbase.dll\0", name) };
        if !p.is_null() && unsafe { patch_fits(p) } && unsafe { crate::net_guard::write_jump(p, hook) } {
            done.push(label);
        } else {
            failed.push(label);
        }
    }
    crate::record_raw("locale", "install", &format!(
        "user language reported as {summary}: {}{}",
        done.join(", "),
        if failed.is_empty() { String::new() } else { format!("; NOT overridden: {}", failed.join(", ")) }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated_and_canonicalised_by_windows() {
        assert_eq!(resolve("en-US"), Ok(Locale { name: "en-US".into(), lcid: 0x0409 }));
        assert_eq!(resolve(" en-us ").unwrap().name, "en-US");
        assert_eq!(resolve("de-DE").unwrap().lcid, 0x0407);
        assert!(resolve("xx-NOTREAL").is_err());
        assert!(resolve("").is_err());
    }

    #[test]
    fn mui_id_form_is_four_hex_digits() {
        let l = Locale { name: "en-US".into(), lcid: 0x0409 };
        assert_eq!((l.langid(), l.id_string()), (0x0409, "0409".to_string()));
    }

    #[test]
    fn buffer_writes_follow_the_windows_contract() {
        let mut b = [0xffffu16; 8];
        assert_eq!(put("en-US", 1, b.as_mut_ptr(), 5), (6, false), "no room for the terminator");
        assert_eq!(b[0], 0xffff, "nothing written when it does not fit");
        assert_eq!(put("en-US", 1, b.as_mut_ptr(), 6), (6, true));
        assert_eq!(&b[..6], &"en-US\0".encode_utf16().collect::<Vec<_>>()[..]);
        assert_eq!(put("en-US", 2, core::ptr::null_mut(), 0), (7, false), "size query");
    }

    #[test]
    fn hooks_report_the_configured_locale() {
        let _ = ACTIVE.set(Locale { name: "en-US".into(), lcid: 0x0409 });
        unsafe {
            let mut b = [0u16; 85];
            assert_eq!(get_user_default_locale_name(b.as_mut_ptr(), 85), 6);
            assert_eq!(String::from_utf16_lossy(&b[..5]), "en-US");
            assert_eq!(get_user_default_locale_name(b.as_mut_ptr(), 3), 0);
            assert_eq!((get_user_default_lcid(), get_user_default_langid()), (0x0409, 0x0409));

            let (mut n, mut cch) = (0u32, 0u32);
            assert_eq!(get_user_preferred_ui_languages(MUI_LANGUAGE_NAME, &mut n, core::ptr::null_mut(), &mut cch), 1);
            assert_eq!((n, cch), (1, 7), "size query: one entry, 'en-US' + two terminators");
            let mut list = [0xffffu16; 7];
            assert_eq!(get_user_preferred_ui_languages(MUI_LANGUAGE_NAME, &mut n, list.as_mut_ptr(), &mut cch), 1);
            assert_eq!(&list, &"en-US\0\0".encode_utf16().collect::<Vec<_>>()[..]);
            let mut small = 3u32;
            assert_eq!(get_user_preferred_ui_languages(MUI_LANGUAGE_ID, &mut n, list.as_mut_ptr(), &mut small), 0);
            assert_eq!(small, 6, "required size reported for the ID form");
            let mut cch = 6u32;
            assert_eq!(get_user_preferred_ui_languages(MUI_LANGUAGE_ID, &mut n, list.as_mut_ptr(), &mut cch), 1);
            assert_eq!(&list[..6], &"0409\0\0".encode_utf16().collect::<Vec<_>>()[..]);
            assert_eq!(get_user_preferred_ui_languages(MUI_LANGUAGE_ID | MUI_LANGUAGE_NAME, &mut n, list.as_mut_ptr(), &mut cch), 0);
        }
    }

    #[test]
    fn host_exports_can_take_the_patch() {
        for name in [&b"GetUserDefaultLocaleName\0"[..], b"GetUserDefaultLCID\0", b"GetUserDefaultLangID\0",
                     b"GetUserDefaultUILanguage\0", b"GetUserPreferredUILanguages\0"] {
            let p = unsafe { crate::net_guard::resolve_export(b"kernelbase.dll\0", name) };
            assert!(!p.is_null(), "{} missing", String::from_utf8_lossy(name));
            assert!(unsafe { patch_fits(p) }, "{}: entry too short for the patch", String::from_utf8_lossy(name));
        }
    }
}
