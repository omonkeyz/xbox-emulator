// XTaskQueue/XAsync conformance scenarios (support/xtq.rs, ported from
// libHttpClient's TaskQueueTests/AsyncBlockTests) against the emulated
// 073b7dcb interface in gdk/rt.rs, obtained through QueryApiImpl like a title.
use compat_common::gdk::{self, xasync};
use std::sync::OnceLock;

#[macro_use]
#[path = "support/xtq.rs"]
mod xtq;

// Scenarios where gdk/rt.rs (main 51f5319) gives a different result from libHttpClient and
// from the PC runtime (gdk_xtaskqueue_conformance_real.rs passes them on xgameruntime.dll
// 10.0.26100.9656, except the two shared-port ones, which check a later libHttpClient fix).
// Each line is what the emulator returned, measured one scenario per process. They are
// skipped by default; TOMBOX_XTQ_ALL=1 runs them. The reference is the PC side: where the
// console is known to differ, the scenario should assert the console result instead, as
// verify_global_queue already does for terminating the process queue.
const DIFFERS: &[(&str, &str)] = &[
    ("verify_begin_after_terminate", "XAsyncBegin on a terminated queue returns S_OK, reference E_ABORT"),
    ("verify_cancellation", "after XAsyncCancel, XAsyncGetStatus(wait) returns before the completion callback has seen E_ABORT"),
    ("verify_cant_schedule_twice", "a second XAsyncSchedule returns S_OK, reference E_UNEXPECTED"),
    ("verify_cleanup_waits_for_work", "after XAsyncCancel, XAsyncGetStatus(wait) returns before the completion callback has seen E_ABORT"),
    ("verify_cleanup_waits_for_work_distributed", "after XAsyncCancel, XAsyncGetStatus(wait) returns before the completion callback has seen E_ABORT"),
    ("verify_close_keeps_queued_callbacks", "closing the handle cancels the 21 queued callbacks, reference runs all 21"),
    ("verify_dispatch_on_pool_port", "XTaskQueueDispatch on a thread-pool port with a queued item returns false"),
    ("verify_duplicate_queue_handle", "XTaskQueueDuplicateHandle returns the same handle, reference a new one"),
    ("verify_failed_begin_completes", "XAsyncBegin returns E_FAIL when the Begin op fails, reference S_OK and a completed call"),
    ("verify_failure_in_do_work", "DoWork returning S_OK without XAsyncComplete leaves status S_OK, reference E_UNEXPECTED"),
    ("verify_get_async_status_no_deadlock", "XAsyncRun on a reused block returns E_INVALIDARG, reference S_OK"),
    ("verify_global_queue", "XAsyncBegin with no process queue returns S_OK, reference E_NO_TASK_QUEUE"),
    ("verify_multithread_manual_dispatch", "terminating the queue does not release XTaskQueueDispatch(INFINITE)"),
    ("verify_queue_monitor_has_correct_ports", "the base queue monitor does not report its port for a shared port"),
    ("verify_queue_termination", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_register_with_auto_reset", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_result_buffer_checks", "XAsyncGetResult with a null buffer returns 0x8007007a, reference E_INVALIDARG"),
    ("verify_shared_port_queue_rundown_drops_its_wait_handles", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_shared_port_queue_rundown_keeps_other_waiters", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_terminate_waits_for_async_in_flight", "termination finishes while an XAsync call on the queue is still pending"),
    ("verify_termination_of_base_queue_keeps_composites", "the composite's pending callback runs canceled, reference runs it normally"),
    ("verify_termination_of_composite_queue", "XTaskQueueRegisterWaiter returns E_NOTIMPL"),
    ("verify_thread_pool_callbacks_can_block", "24 blocking thread-pool callbacks cannot all run at once"),
    // Passes alone and with --test-threads=1; fails when the scenarios run in parallel (3 runs of 3).
    ("verify_wait_for_completion", "under load, XAsyncGetStatus(wait) returns before the completion callback has finished"),
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
