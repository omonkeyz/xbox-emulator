use core::ffi::c_void;

#[cfg(test)]
mod tests;

pub const FFX_ERROR_INVALID_POINTER: i32 = -1;
pub const FFX_ERROR_BACKEND_API_ERROR: i32 = -2;

#[no_mangle]
pub unsafe extern "system" fn ffxCreateContext(
    context: *mut *mut c_void,
    _header: *const c_void,
    _mem_cb: *const c_void,
) -> i32 {
    if context.is_null() {
        return FFX_ERROR_INVALID_POINTER;
    }
    *context = core::ptr::null_mut();
    FFX_ERROR_BACKEND_API_ERROR
}

#[no_mangle]
pub unsafe extern "system" fn ffxDestroyContext(
    context: *mut *mut c_void,
    _mem_cb: *const c_void,
) -> i32 {
    if context.is_null() {
        return FFX_ERROR_INVALID_POINTER;
    }
    *context = core::ptr::null_mut();
    FFX_ERROR_BACKEND_API_ERROR
}

#[no_mangle]
pub unsafe extern "system" fn ffxDispatch(
    context: *mut c_void,
    _header: *const c_void,
) -> i32 {
    if context.is_null() {
        return FFX_ERROR_INVALID_POINTER;
    }
    FFX_ERROR_BACKEND_API_ERROR
}

#[no_mangle]
pub unsafe extern "system" fn ffxQuery(
    context: *mut c_void,
    _header: *const c_void,
) -> i32 {
    if context.is_null() {
        return FFX_ERROR_INVALID_POINTER;
    }
    FFX_ERROR_BACKEND_API_ERROR
}
