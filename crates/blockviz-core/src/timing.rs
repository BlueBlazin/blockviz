//! How long each stage of an analysis took, for the CLI and the native app:
//! `stage("parse Python")` ends the stage that ran since the previous mark.
//! WebAssembly has no clock of its own, so nothing is measured there.

#[cfg(not(target_arch = "wasm32"))]
mod clock {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    thread_local! {
        static STAGES: RefCell<(Option<Instant>, Vec<(&'static str, Duration)>)> = const { RefCell::new((None, Vec::new())) };
    }

    pub fn start() {
        STAGES.with(|s| *s.borrow_mut() = (Some(Instant::now()), Vec::new()));
    }

    pub fn stage(name: &'static str) {
        STAGES.with(|s| {
            let (since, stages) = &mut *s.borrow_mut();
            let now = Instant::now();
            if let Some(t) = since.replace(now) {
                stages.push((name, now - t));
            }
        });
    }

    pub fn take() -> Vec<(&'static str, Duration)> {
        STAGES.with(|s| std::mem::take(&mut s.borrow_mut().1))
    }
}

#[cfg(target_arch = "wasm32")]
mod clock {
    pub fn start() {}
    pub fn stage(_: &'static str) {}
    pub fn take() -> Vec<(&'static str, std::time::Duration)> {
        Vec::new()
    }
}

pub use clock::{stage, start, take};
