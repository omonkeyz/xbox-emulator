//! In-process console LSA policy state.
//!
//! The console `LsaEnablePasswordLessCurrentUser` and
//! `LsaDisablePasswordLessCurrentUser` exports are no-argument `lsarpc`
//! `NdrClientCall3` wrappers (opnums 0x7e and 0x7f in the 19041/22000 images).
//! The emulator must not change the signed-in PC user's sign-in options, so the
//! console operation is modelled as process-local state instead. No host LSA
//! account, registry key or policy setting is read or written.

use core::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Apply the console enable/disable operation to the emulated user.
pub fn set(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
}

/// The emulated passwordless state for the current process.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_process_local() {
        set(true);
        assert!(enabled());
        set(false);
        assert!(!enabled());
    }
}
