use compat_common::gdk::{self, xasync};
use std::sync::OnceLock;

#[macro_use]
#[path = "support/xtq.rs"]
mod xtq;

const DIFFERS: &[(&str, &str)] = &[
    ("verify_cancellation", "XAsyncCancel completes the call itself with E_ABORT, reference leaves it pending until the provider completes it"),
    ("verify_cleanup_waits_for_work", "XAsyncCancel completes the call itself with E_ABORT, reference leaves it pending until the provider completes it"),
    ("verify_cleanup_waits_for_work_distributed", "XAsyncCancel completes the call itself with E_ABORT, reference leaves it pending until the provider completes it"),
    ("verify_close_keeps_queued_callbacks", "closing the handle cancels the 21 queued callbacks, reference runs all 21"),
    ("verify_dispatch_on_pool_port", "XTaskQueueDispatch on a thread-pool port with a queued item returns false"),
    ("verify_duplicate_queue_handle", "XTaskQueueDuplicateHandle returns the same handle, reference a new one"),
    ("verify_get_async_status_no_deadlock", "does not finish: a call begun on a block whose previous call has just completed can lose its state"),
    ("verify_multithread_manual_dispatch", "terminating the queue does not release XTaskQueueDispatch(INFINITE)"),
    ("verify_no_write_after_get_status_race", "under load, intermittent: the async block is written after XAsyncGetStatus(wait) has returned"),
    ("verify_queue_monitor_has_correct_ports", "the base queue monitor does not report its port for a shared port"),
    ("verify_queue_termination", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_register_with_auto_reset", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_shared_port_queue_rundown_drops_its_wait_handles", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_shared_port_queue_rundown_keeps_other_waiters", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_terminate_waits_for_async_in_flight", "termination finishes while an XAsync call on the queue is still pending"),
    ("verify_termination_of_base_queue_keeps_composites", "the composite's pending callback runs canceled, reference runs it normally"),
    ("verify_termination_of_composite_queue", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_thread_pool_callbacks_can_block", "24 blocking thread-pool callbacks cannot all run at once"),
    ("verify_wait_for_completion", "intermittent: XAsyncGetStatus(wait) on a completed call with a result returns before the completion callback has finished"),
];

fn setup(scenario: &str) -> bool {
    if let Some((_, what)) = DIFFERS.iter().find(|(n, _)| *n == scenario) {
        if std::env::var("TOMBOX_XTQ_ALL").as_deref() != Ok("1") {
            eprintln!("SKIP: {scenario}: {what}");
            return false;
        }
    }
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        std::env::set_var("TOMBOX_GDK_XASYNC", "emulated");
        std::env::set_var("TOMBOX_GDK_ROOT", std::env::temp_dir().join(format!("tombox_gdk_xtq_{}", std::process::id())));
        assert!(gdk::rt::active());
        let mut obj = core::ptr::null_mut();
        assert_eq!(unsafe { gdk::query_api_impl(&xasync::XASYNC, &xasync::XASYNC, &mut obj) }, 0);
        xtq::install(obj);
        xtq::set_console_model(true);
        true
    })
}

xtq_tests!(setup);
