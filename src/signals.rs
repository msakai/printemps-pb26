use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use signal_hook::consts::{SIGINT, SIGTERM, SIGXCPU};
use signal_hook::iterator::Signals;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Default)]
pub struct ChildSlot {
    inner: Arc<Mutex<Option<Pid>>>,
}

impl ChildSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, pid: Option<Pid>) {
        *self.inner.lock().unwrap() = pid;
    }

    pub fn clear(&self) {
        self.set(None);
    }

    fn get(&self) -> Option<Pid> {
        *self.inner.lock().unwrap()
    }
}

#[derive(Clone, Default)]
pub struct InterruptFlag {
    inner: Arc<AtomicBool>,
}

impl InterruptFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_set(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }

    fn set(&self) {
        self.inner.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::unistd::Pid;

    #[test]
    fn child_slot_initial_empty() {
        assert!(ChildSlot::new().get().is_none());
    }

    #[test]
    fn child_slot_set_and_get() {
        let slot = ChildSlot::new();
        slot.set(Some(Pid::from_raw(42)));
        assert_eq!(slot.get(), Some(Pid::from_raw(42)));
    }

    #[test]
    fn child_slot_clear() {
        let slot = ChildSlot::new();
        slot.set(Some(Pid::from_raw(1)));
        slot.clear();
        assert!(slot.get().is_none());
    }

    #[test]
    fn child_slot_clone_shares_state() {
        let slot = ChildSlot::new();
        let clone = slot.clone();
        slot.set(Some(Pid::from_raw(99)));
        assert_eq!(clone.get(), Some(Pid::from_raw(99)));
    }

    #[test]
    fn interrupt_flag_initial_false() {
        assert!(!InterruptFlag::new().is_set());
    }

    #[test]
    fn interrupt_flag_set() {
        let flag = InterruptFlag::new();
        flag.set();
        assert!(flag.is_set());
    }

    #[test]
    fn interrupt_flag_clone_shares_state() {
        let flag = InterruptFlag::new();
        let clone = flag.clone();
        flag.set();
        assert!(clone.is_set());
    }
}

/// Install handlers for SIGINT, SIGTERM, SIGXCPU. Each received signal is
/// forwarded to whichever child PID is currently registered in the slot.
///
/// The handler thread runs for the lifetime of the process. The returned
/// `ChildSlot` should be updated as new child processes are spawned.
pub fn install_forwarder() -> (ChildSlot, InterruptFlag) {
    let slot = ChildSlot::new();
    let flag = InterruptFlag::new();
    let slot_for_thread = slot.clone();
    let flag_for_thread = flag.clone();

    let mut signals =
        Signals::new([SIGINT, SIGTERM, SIGXCPU]).expect("failed to install signal handlers");

    thread::Builder::new()
        .name("pb-signal-forwarder".to_string())
        .spawn(move || {
            for sig in signals.forever() {
                let nix_sig = match sig {
                    SIGINT => Signal::SIGINT,
                    SIGTERM => Signal::SIGTERM,
                    SIGXCPU => Signal::SIGXCPU,
                    _ => continue,
                };
                flag_for_thread.set();
                if let Some(pid) = slot_for_thread.get() {
                    let _ = signal::kill(pid, nix_sig);
                }
            }
        })
        .expect("failed to spawn signal forwarder thread");

    (slot, flag)
}
