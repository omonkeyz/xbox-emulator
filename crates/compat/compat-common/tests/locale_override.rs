use std::ptr;

#[link(name = "kernel32")]
extern "system" {
    fn GetUserDefaultLocaleName(buf: *mut u16, cch: i32) -> i32;
    fn GetUserDefaultLCID() -> u32;
    fn GetLastError() -> u32;
}

#[test]
fn public_locale_name_supports_query_then_fill() {
    let locale = if unsafe { GetUserDefaultLCID() } == 0x0407 {
        "en-US"
    } else {
        "de-DE"
    };
    compat_common::locale::install(locale);

    let expected: Vec<_> = locale.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let needed = GetUserDefaultLocaleName(ptr::null_mut(), 0);
        assert_eq!(needed, expected.len() as i32);
        let mut buf = vec![0xffff; needed as usize];
        assert_eq!(GetUserDefaultLocaleName(buf.as_mut_ptr(), needed - 1), 0);
        assert_eq!(GetLastError(), 122); // ERROR_INSUFFICIENT_BUFFER
        assert_eq!(GetUserDefaultLocaleName(buf.as_mut_ptr(), needed), needed);
        assert_eq!(buf, expected);
    }
}
