use core::ffi::c_void;

#[cfg(test)]
mod tests;

pub const S_OK: i32 = 0;
pub const E_NOTIMPL: i32 = -2147467263;
pub const E_POINTER: i32 = -2147467261;
pub const E_NOINTERFACE: i32 = -2147467262;

pub const IID_IUNKNOWN: [u8; 16] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

#[repr(C)]
pub struct BvhPrebuildInfo {
    pub result_data_max_size_in_bytes: u64,
    pub scratch_data_size_in_bytes: u64,
    pub update_scratch_data_size_in_bytes: u64,
}

#[repr(C)]
pub struct IXboxBvhBuilderVtbl {
    pub query_interface: unsafe extern "system" fn(*mut XboxBvhBuilder, *const [u8; 16], *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "system" fn(*mut XboxBvhBuilder) -> u32,
    pub release: unsafe extern "system" fn(*mut XboxBvhBuilder) -> u32,
    pub get_prebuild_info: unsafe extern "system" fn(*mut XboxBvhBuilder, *const c_void, *mut BvhPrebuildInfo) -> i32,
    pub build_bottom_level_as: unsafe extern "system" fn(*mut XboxBvhBuilder, *mut c_void, *const c_void) -> i32,
    pub build_top_level_as: unsafe extern "system" fn(*mut XboxBvhBuilder, *mut c_void, *const c_void) -> i32,
    pub copy_as: unsafe extern "system" fn(*mut XboxBvhBuilder, *mut c_void, u64, u64, u32) -> i32,
}

#[repr(C)]
pub struct XboxBvhBuilder {
    pub vtbl: *const IXboxBvhBuilderVtbl,
    pub ref_count: u32,
    pub device: *mut c_void,
}

static VTBL: IXboxBvhBuilderVtbl = IXboxBvhBuilderVtbl {
    query_interface,
    add_ref,
    release,
    get_prebuild_info,
    build_bottom_level_as,
    build_top_level_as,
    copy_as,
};

unsafe extern "system" fn query_interface(this: *mut XboxBvhBuilder, riid: *const [u8; 16], out: *mut *mut c_void) -> i32 {
    if this.is_null() || riid.is_null() || out.is_null() {
        return E_POINTER;
    }
    if *riid == IID_IUNKNOWN {
        add_ref(this);
        *out = this as *mut c_void;
        S_OK
    } else {
        *out = core::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut XboxBvhBuilder) -> u32 {
    if this.is_null() {
        return 0;
    }
    (*this).ref_count = (*this).ref_count.saturating_add(1);
    (*this).ref_count
}

unsafe extern "system" fn release(this: *mut XboxBvhBuilder) -> u32 {
    if this.is_null() {
        return 0;
    }
    (*this).ref_count = (*this).ref_count.saturating_sub(1);
    let count = (*this).ref_count;
    if count == 0 {
        drop(Box::from_raw(this));
    }
    count
}

unsafe extern "system" fn get_prebuild_info(_this: *mut XboxBvhBuilder, _inputs: *const c_void, out: *mut BvhPrebuildInfo) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    (*out).result_data_max_size_in_bytes = 0;
    (*out).scratch_data_size_in_bytes = 0;
    (*out).update_scratch_data_size_in_bytes = 0;
    E_NOTIMPL
}

unsafe extern "system" fn build_bottom_level_as(_this: *mut XboxBvhBuilder, _cmd_list: *mut c_void, _desc: *const c_void) -> i32 {
    E_NOTIMPL
}

unsafe extern "system" fn build_top_level_as(_this: *mut XboxBvhBuilder, _cmd_list: *mut c_void, _desc: *const c_void) -> i32 {
    E_NOTIMPL
}

unsafe extern "system" fn copy_as(_this: *mut XboxBvhBuilder, _cmd_list: *mut c_void, _dst: u64, _src: u64, _mode: u32) -> i32 {
    E_NOTIMPL
}

#[no_mangle]
pub unsafe extern "system" fn CreateXboxBvhBuilder(device: *mut c_void, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    let builder = Box::into_raw(Box::new(XboxBvhBuilder {
        vtbl: &VTBL,
        ref_count: 1,
        device,
    }));
    *out = builder as *mut c_void;
    S_OK
}

#[no_mangle]
pub unsafe extern "system" fn XboxBvhBuilderInitialize(this: *mut c_void, device: *mut c_void) -> i32 {
    if this.is_null() {
        return E_POINTER;
    }
    let b = this as *mut XboxBvhBuilder;
    (*b).device = device;
    S_OK
}

#[no_mangle]
pub unsafe extern "system" fn XboxBvhBuilderGetPrebuildInfo(this: *mut c_void, inputs: *const c_void, out_info: *mut BvhPrebuildInfo) -> i32 {
    if this.is_null() || out_info.is_null() {
        return E_POINTER;
    }
    let b = this as *mut XboxBvhBuilder;
    ((*(*b).vtbl).get_prebuild_info)(b, inputs, out_info)
}

#[no_mangle]
pub unsafe extern "system" fn XboxBvhBuilderBuildBottomLevel(this: *mut c_void, cmd_list: *mut c_void, desc: *const c_void) -> i32 {
    if this.is_null() {
        return E_POINTER;
    }
    let b = this as *mut XboxBvhBuilder;
    ((*(*b).vtbl).build_bottom_level_as)(b, cmd_list, desc)
}

#[no_mangle]
pub unsafe extern "system" fn XboxBvhBuilderBuildTopLevel(this: *mut c_void, cmd_list: *mut c_void, desc: *const c_void) -> i32 {
    if this.is_null() {
        return E_POINTER;
    }
    let b = this as *mut XboxBvhBuilder;
    ((*(*b).vtbl).build_top_level_as)(b, cmd_list, desc)
}

#[no_mangle]
pub unsafe extern "system" fn XboxBvhBuilderCopy(this: *mut c_void, cmd_list: *mut c_void, dst: u64, src: u64, mode: u32) -> i32 {
    if this.is_null() {
        return E_POINTER;
    }
    let b = this as *mut XboxBvhBuilder;
    ((*(*b).vtbl).copy_as)(b, cmd_list, dst, src, mode)
}

#[no_mangle]
pub unsafe extern "system" fn XboxBvhBuilderDestroy(this: *mut c_void) -> i32 {
    if this.is_null() {
        return E_POINTER;
    }
    let b = this as *mut XboxBvhBuilder;
    ((*(*b).vtbl).release)(b);
    S_OK
}
