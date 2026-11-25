use super::*;

#[test]
fn test_xbox_bvh_builder_lifecycle() {
    unsafe {
        let mut builder_ptr: *mut c_void = core::ptr::null_mut();
        assert_eq!(CreateXboxBvhBuilder(core::ptr::null_mut(), &mut builder_ptr), S_OK);
        assert!(!builder_ptr.is_null());
        assert_eq!(XboxBvhBuilderInitialize(builder_ptr, core::ptr::null_mut()), S_OK);
        let mut info = BvhPrebuildInfo {
            result_data_max_size_in_bytes: 0,
            scratch_data_size_in_bytes: 0,
            update_scratch_data_size_in_bytes: 0,
        };
        assert_eq!(XboxBvhBuilderGetPrebuildInfo(builder_ptr, core::ptr::null(), &mut info), E_NOTIMPL);
        assert_eq!(info.result_data_max_size_in_bytes, 0);
        assert_eq!(XboxBvhBuilderBuildBottomLevel(builder_ptr, core::ptr::null_mut(), core::ptr::null()), E_NOTIMPL);
        assert_eq!(XboxBvhBuilderBuildTopLevel(builder_ptr, core::ptr::null_mut(), core::ptr::null()), E_NOTIMPL);
        assert_eq!(XboxBvhBuilderCopy(builder_ptr, core::ptr::null_mut(), 0, 0, 0), E_NOTIMPL);
        assert_eq!(XboxBvhBuilderDestroy(builder_ptr), S_OK);
    }
}

#[test]
fn test_com_interface() {
    unsafe {
        let mut builder_ptr: *mut c_void = core::ptr::null_mut();
        assert_eq!(CreateXboxBvhBuilder(core::ptr::null_mut(), &mut builder_ptr), S_OK);
        let b = builder_ptr as *mut XboxBvhBuilder;
        assert_eq!(((*(*b).vtbl).add_ref)(b), 2);
        assert_eq!(((*(*b).vtbl).release)(b), 1);
        let mut unk: *mut c_void = core::ptr::null_mut();
        assert_eq!(((*(*b).vtbl).query_interface)(b, &IID_IUNKNOWN, &mut unk), S_OK);
        assert_eq!(unk, builder_ptr);
        assert_eq!(((*(*b).vtbl).release)(b), 1);
        assert_eq!(((*(*b).vtbl).release)(b), 0);
    }
}

#[test]
fn test_null_pointers() {
    unsafe {
        assert_eq!(CreateXboxBvhBuilder(core::ptr::null_mut(), core::ptr::null_mut()), E_POINTER);
        assert_eq!(XboxBvhBuilderInitialize(core::ptr::null_mut(), core::ptr::null_mut()), E_POINTER);
        assert_eq!(XboxBvhBuilderGetPrebuildInfo(core::ptr::null_mut(), core::ptr::null(), core::ptr::null_mut()), E_POINTER);
        assert_eq!(XboxBvhBuilderBuildBottomLevel(core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null()), E_POINTER);
        assert_eq!(XboxBvhBuilderBuildTopLevel(core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null()), E_POINTER);
        assert_eq!(XboxBvhBuilderCopy(core::ptr::null_mut(), core::ptr::null_mut(), 0, 0, 0), E_POINTER);
        assert_eq!(XboxBvhBuilderDestroy(core::ptr::null_mut()), E_POINTER);
    }
}
