use core::ffi::c_void;

#[cfg(test)]
mod tests;

#[repr(C)]
pub struct Bink {
    _opaque: [u8; 0],
}

#[no_mangle]
pub unsafe extern "system" fn BinkOpen(_name: *const c_void, _flags: u32) -> *mut Bink {
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "system" fn BinkClose(_bink: *mut Bink) {}

#[no_mangle]
pub unsafe extern "system" fn BinkDoFramePlane(_bink: *mut Bink, _plane: u32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkFreeGlobals() {}

#[no_mangle]
pub unsafe extern "system" fn BinkGetFrameBuffersInfo(_bink: *mut Bink, _info: *mut c_void) {}

#[no_mangle]
pub unsafe extern "system" fn BinkGetKeyFrame(_bink: *mut Bink, _frame: u32, _flags: u32) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkGetRealtime(_bink: *mut Bink, _run: *mut u32, _frames: *mut u32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkGetTrackID(_bink: *mut Bink, _track_index: u32) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkGoto(_bink: *mut Bink, _frame: u32, _flags: u32) {}

#[no_mangle]
pub unsafe extern "system" fn BinkNextFrame(_bink: *mut Bink) {}

#[no_mangle]
pub unsafe extern "system" fn BinkPause(_bink: *mut Bink, _pause: i32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkRegisterFrameBuffers(_bink: *mut Bink, _info: *mut c_void) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkSetFileOffset(_offset: u64) {}

#[no_mangle]
pub unsafe extern "system" fn BinkSetFrameRate(_rate: u32, _scale: u32) {}

#[no_mangle]
pub unsafe extern "system" fn BinkSetMemory(_alloc_fn: *const c_void, _free_fn: *const c_void) {}

#[no_mangle]
pub unsafe extern "system" fn BinkSetOSFileCallbacks(_open: *const c_void, _read: *const c_void, _close: *const c_void) {}

#[no_mangle]
pub unsafe extern "system" fn BinkSetSoundSystem(_init_fn: *const c_void, _param: *const c_void) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkSetSoundTrack(_total: u32, _ids: *const u32) {}

#[no_mangle]
pub unsafe extern "system" fn BinkSetSpeakerVolumes(_bink: *mut Bink, _track: u32, _vols: *const f32, _total_vols: u32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkSetVolume(_bink: *mut Bink, _track: u32, _volume: i32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkSetWillLoop(_bink: *mut Bink, _loop_flag: i32) {}

#[no_mangle]
pub unsafe extern "system" fn BinkShouldSkip(_bink: *mut Bink) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn BinkWait(_bink: *mut Bink) -> i32 {
    0
}
