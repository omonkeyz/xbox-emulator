use super::*;

#[test]
fn test_bink_open_fails_honestly() {
    unsafe {
        let b = BinkOpen(core::ptr::null(), 0);
        assert!(b.is_null());
        assert_eq!(BinkGetKeyFrame(b, 10, 0), 0);
        assert_eq!(BinkGetTrackID(b, 3), 0);
        assert_eq!(BinkDoFramePlane(b, 0), 0);
        assert_eq!(BinkGetRealtime(b, core::ptr::null_mut(), core::ptr::null_mut()), 0);
        assert_eq!(BinkRegisterFrameBuffers(b, core::ptr::null_mut()), 0);
        assert_eq!(BinkSetSoundSystem(core::ptr::null(), core::ptr::null()), 0);
        assert_eq!(BinkSetSpeakerVolumes(b, 0, core::ptr::null(), 0), 0);
        assert_eq!(BinkSetVolume(b, 0, 100), 0);
        assert_eq!(BinkShouldSkip(b), 0);
        assert_eq!(BinkWait(b), 0);
        BinkGetFrameBuffersInfo(b, core::ptr::null_mut());
        BinkSetFileOffset(0);
        BinkSetFrameRate(30, 1);
        BinkSetMemory(core::ptr::null(), core::ptr::null());
        BinkSetOSFileCallbacks(core::ptr::null(), core::ptr::null(), core::ptr::null());
        BinkSetSoundTrack(0, core::ptr::null());
        BinkSetWillLoop(b, 1);
        BinkFreeGlobals();
        BinkClose(b);
    }
}

#[test]
fn test_null_bink_safe() {
    unsafe {
        BinkClose(core::ptr::null_mut());
        BinkGoto(core::ptr::null_mut(), 0, 0);
        BinkNextFrame(core::ptr::null_mut());
        assert_eq!(BinkPause(core::ptr::null_mut(), 1), 0);
        assert_eq!(BinkDoFramePlane(core::ptr::null_mut(), 0), 0);
        assert_eq!(BinkShouldSkip(core::ptr::null_mut()), 0);
        assert_eq!(BinkWait(core::ptr::null_mut()), 0);
    }
}
