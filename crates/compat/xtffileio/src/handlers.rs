use crate::com::*;
use crate::file;
use crate::msg::*;
use crate::paths::*;
use core::ffi::c_void;

unsafe fn bstr_wide<'a>(b: &Bstr) -> &'a [u16] {
    if b.0.is_null() {
        return &[];
    }
    let mut n = 0;
    while *b.0.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(b.0, n)
}

pub unsafe fn local_path(name: &Bstr) -> Result<(u32, String), HRESULT> {
    let w = bstr_wide(name);
    let (t, rel) = path_type(w)?;
    let rel = String::from_utf16_lossy(&w[rel..]);
    Ok((t, rel))
}

pub unsafe fn resolve_local(name: &Bstr) -> Result<String, HRESULT> {
    let (t, rel) = local_path(name)?;
    if t == PATH_APPLICATION {
        return Err(HR_FILE_NOT_FOUND);
    }
    full_path_name(&rel)
}

unsafe fn run<Req: Props + Default + HasFileName>(request: *mut c_void, op: impl FnOnce(&Req, &str) -> HRESULT) -> HRESULT {
    let mut req = Req::default();
    let hr = deserialize(&mut req, request);
    if hr < 0 {
        return hr;
    }
    let name = file_name_of(&req);
    match resolve_local(name) {
        Ok(full) => op(&req, &full),
        Err(e) => e,
    }
}

pub trait HasFileName {
    fn file_name(&self) -> &Bstr;
}

impl HasFileName for MsgFileName {
    fn file_name(&self) -> &Bstr {
        &self.file_name
    }
}

impl HasFileName for MsgFlags {
    fn file_name(&self) -> &Bstr {
        &self.base.file_name
    }
}

impl HasFileName for MsgModifyFileAttributes {
    fn file_name(&self) -> &Bstr {
        &self.base.file_name
    }
}

impl HasFileName for MsgSetFileAttributes {
    fn file_name(&self) -> &Bstr {
        &self.base.file_name
    }
}

impl HasFileName for MsgFindFiles {
    fn file_name(&self) -> &Bstr {
        &self.base.base.file_name
    }
}

fn file_name_of<T: HasFileName>(r: &T) -> &Bstr {
    r.file_name()
}

unsafe fn reply_result(hr: HRESULT, response: *mut c_void) -> HRESULT {
    serialize(&MsgResponse { result: hr }, response)
}

pub unsafe extern "system" fn create_directory_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let hr = run::<MsgFileName>(request, |_, p| file::create_directory(p));
    reply_result(hr, response)
}

pub unsafe extern "system" fn delete_file_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let hr = run::<MsgFlags>(request, |r, p| file::delete_files(p, r.flags));
    reply_result(hr, response)
}

pub unsafe extern "system" fn get_file_attributes_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let mut resp = MsgGetFileAttributesResponse::default();
    resp.base.result = run::<MsgFileName>(request, |_, p| match file::get_file_attributes(p) {
        Ok(a) => {
            resp.attributes = a;
            S_OK
        }
        Err(e) => e,
    });
    serialize(&resp, response)
}

pub unsafe extern "system" fn get_file_size_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let mut resp = MsgGetFileSizeResponse::default();
    resp.base.result = run::<MsgFileName>(request, |_, p| match file::get_file_size(p) {
        Ok(s) => {
            resp.size = s;
            S_OK
        }
        Err(e) => e,
    });
    serialize(&resp, response)
}

pub unsafe extern "system" fn modify_file_attributes_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let hr = run::<MsgModifyFileAttributes>(request, |r, p| file::modify_file_attributes(p, r.add, r.remove));
    reply_result(hr, response)
}

pub unsafe extern "system" fn remove_directory_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let hr = run::<MsgFlags>(request, |r, p| file::remove_directory(p, r.flags));
    reply_result(hr, response)
}

pub unsafe extern "system" fn set_file_attributes_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let hr = run::<MsgSetFileAttributes>(request, |r, p| file::set_file_attributes(p, r.attributes));
    reply_result(hr, response)
}

pub fn file_info_json(hr: HRESULT, info: &file::FileInfo) -> String {
    let t = |v: u64| format!("{{\"L\":{},\"H\":{}}}", v as u32, (v >> 32) as u32 as i32);
    format!(
        "{{\"Result\":{},\"FileAttributes\":{},\"CreationTime\":{},\"LastAccessTime\":{},\"LastWriteTime\":{},\"FileSize\":{}}}",
        hr,
        info.attributes,
        t(info.creation),
        t(info.access),
        t(info.write),
        t(info.size)
    )
}

pub const FILE_INFO_BUFFER: usize = 0xf4;

pub unsafe extern "system" fn get_file_info_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let mut info = file::FileInfo::default();
    let hr = run::<MsgFileName>(request, |_, p| match file::get_file_info(p) {
        Ok(i) => {
            info = i;
            S_OK
        }
        Err(e) => e,
    });
    let w = wide(&file_info_json(hr, &info));
    (vt::<MessageVtbl>(response).set_json_body_w)(response, w.as_ptr())
}

unsafe fn application_path(request: *mut c_void) -> Result<(), HRESULT> {
    let mut req = MsgFileName::default();
    let hr = deserialize(&mut req, request);
    if hr < 0 {
        return Err(hr);
    }
    match local_path(&req.file_name)? {
        (PATH_APPLICATION, _) => Ok(()),
        _ => Err(E_INVALIDARG),
    }
}

unsafe fn mount_result(request: *mut c_void) -> HRESULT {
    match application_path(request) {
        Ok(()) => HR_FILE_NOT_FOUND,
        Err(e) => e,
    }
}

pub unsafe extern "system" fn application_operations_start_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    reply_result(mount_result(request), response)
}

pub unsafe extern "system" fn application_operations_end_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    reply_result(mount_result(request), response)
}

pub unsafe extern "system" fn package_mount_hold_handler(session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let mut hr = match application_path(request) {
        Ok(()) => S_OK,
        Err(e) => e,
    };
    if hr >= 0 {
        let mut msg = Com::null();
        hr = XbtpCreateMessage(&IID_XBTP_MESSAGE_IMPL, msg.out());
        if hr >= 0 {
            hr = serialize_impl(&MsgResponse { result: HR_FILE_NOT_FOUND }, msg.0);
        }
        if hr >= 0 {
            let s = session as *mut c_void;
            hr = if s.is_null() { E_INVALIDARG } else { (vt::<SessionVtbl>(s).send_more_message)(s, msg.0) };
        }
        if hr >= 0 {
            hr = HR_FILE_NOT_FOUND;
        }
    }
    reply_result(hr, response)
}

pub unsafe extern "system" fn get_storage_device_free_space_handler(_session: usize, request: *mut c_void, response: *mut c_void) -> HRESULT {
    let mut req = MsgStorageDevice::default();
    let hr = deserialize(&mut req, request);
    let resp = MsgStorageDeviceFreeSpaceResponse { base: MsgResponse { result: hr }, total: 0, free: 0 };
    serialize(&resp, response)
}
