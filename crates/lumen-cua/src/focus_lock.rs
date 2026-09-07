//! Process-wide Act focus lock. Only one replay may borrow focus at a time.

use std::sync::atomic::{AtomicBool, Ordering};

static HELD: AtomicBool = AtomicBool::new(false);

pub fn is_held() -> bool {
    HELD.load(Ordering::SeqCst)
}

pub struct FocusGuard {
    held: bool,
}

impl FocusGuard {
    pub fn try_acquire() -> Option<Self> {
        if HELD
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(Self { held: true })
        } else {
            None
        }
    }
}

impl Drop for FocusGuard {
    fn drop(&mut self) {
        if self.held {
            HELD.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_drop() {
        let first = FocusGuard::try_acquire().expect("lock should be free");
        assert!(FocusGuard::try_acquire().is_none());
        drop(first);
        assert!(FocusGuard::try_acquire().is_some());
    }
}
