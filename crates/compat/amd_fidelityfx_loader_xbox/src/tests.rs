use super::*;

#[test]
fn test_fidelityfx_honest_failure() {
    unsafe {
        let mut ctx: *mut c_void = 0x1usize as *mut c_void;
        assert_eq!(ffxCreateContext(&mut ctx, core::ptr::null(), core::ptr::null()), FFX_ERROR_BACKEND_API_ERROR);
        assert!(ctx.is_null());
        assert_eq!(ffxDispatch(ctx, core::ptr::null()), FFX_ERROR_INVALID_POINTER);
        assert_eq!(ffxQuery(ctx, core::ptr::null()), FFX_ERROR_INVALID_POINTER);
        let dummy_ctx = 0x1usize as *mut c_void;
        assert_eq!(ffxDispatch(dummy_ctx, core::ptr::null()), FFX_ERROR_BACKEND_API_ERROR);
        assert_eq!(ffxQuery(dummy_ctx, core::ptr::null()), FFX_ERROR_BACKEND_API_ERROR);
        assert_eq!(ffxDestroyContext(&mut ctx, core::ptr::null()), FFX_ERROR_BACKEND_API_ERROR);
        assert!(ctx.is_null());
    }
}

#[test]
fn test_null_pointers() {
    unsafe {
        assert_eq!(ffxCreateContext(core::ptr::null_mut(), core::ptr::null(), core::ptr::null()), FFX_ERROR_INVALID_POINTER);
        assert_eq!(ffxDestroyContext(core::ptr::null_mut(), core::ptr::null()), FFX_ERROR_INVALID_POINTER);
    }
}
