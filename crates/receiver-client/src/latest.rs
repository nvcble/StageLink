//! A one-slot mailbox: the newest value wins, and the reader never waits.
//!
//! Three things need exactly this shape, and all three are read from a thread
//! that must not block — the audio callback picking up the performer's
//! settings and buffer preset, and the network thread picking up a new session
//! key. `try_lock` rather than `lock`: a missed take costs one block of delay
//! in applying a change, where waiting would cost a dropout.
//!
//! Only the latest value matters in every case. A performer dragging a volume
//! fader produces dozens of changes a second and only the one they stopped at
//! is interesting, so this overwrites rather than queueing.

use parking_lot::Mutex;

pub struct Latest<T>(pub(crate) Mutex<Option<T>>);

// Written out rather than derived: `derive(Default)` would demand
// `T: Default`, and an empty slot needs nothing of the kind.
impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<T> Latest<T> {
    /// Replaces whatever was pending. Called from the app thread.
    pub fn post(&self, value: T) {
        *self.0.lock() = Some(value);
    }

    /// Takes the pending value, or nothing. Never waits.
    pub fn take(&self) -> Option<T> {
        self.0.try_lock().and_then(|mut slot| slot.take())
    }
}
