//! Channels whose senders wake the main loop, so commands and scan events are handled right
//! away instead of on the next timer tick.
use gtk::glib;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

static HOOK: OnceLock<fn()> = OnceLock::new();
static PENDING: AtomicBool = AtomicBool::new(false);

/// The function that drains the channels on the main thread.
pub fn set_hook(hook: fn()) {
    let _ = HOOK.set(hook);
}

/// Called by the hook before draining, so sends made meanwhile schedule a new wake-up.
pub fn drained() {
    PENDING.store(false, Ordering::Release);
}

/// Schedule another drain, for work left after a bounded batch.
pub fn rearm() {
    wake();
}

fn wake() {
    if let Some(hook) = HOOK.get().copied() {
        // Coalesce bursts; always deferred (never run inside the sender's own call).
        if !PENDING.swap(true, Ordering::AcqRel) {
            glib::idle_add_once(hook);
        }
    }
}

pub struct Sender<T>(mpsc::Sender<T>);

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let result = self.0.send(value);
        wake();
        result
    }
}

pub fn channel<T>() -> (Sender<T>, mpsc::Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Sender(tx), rx)
}
