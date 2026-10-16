//! AutoSave that writes off the UI thread. The UI thread stamps and clones the document (the
//! recovery worker's pattern) and hands the copy here; a worker thread serializes and writes
//! it, so typing never waits for the disk. Only one write runs at a time — while one is in
//! flight the app skips AutoSave ticks and tries again when its outcome is read
//! ([`AutoSaveWorker::poll`]). The engine's safe save (sibling temp file, then rename) keeps
//! the destination file intact even when a write races an explicit save or the process ends.

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, TryRecvError};
    use std::time::Duration;

    use wordcraft_engine::cmd::file::{PreparedSave, write_prepared};

    /// How a background AutoSave ended: the file written, or the path and the reason it wasn't.
    pub type Outcome = (PathBuf, Result<(), String>);

    /// A background AutoSave writer, driven like the recovery worker: hand it a job, read the
    /// outcome later. When the worker is gone (spawn failed, or the app is shutting down),
    /// `submit` is a no-op and `is_busy` stays false, so AutoSave falls back to writing inline.
    pub struct AutoSaveWorker {
        tx: std::sync::mpsc::Sender<PreparedSave>,
        events: Receiver<Outcome>,
        busy: bool,
    }

    impl AutoSaveWorker {
        pub fn spawn() -> Self {
            let (tx, jobs) = std::sync::mpsc::channel::<PreparedSave>();
            let (events, rx) = std::sync::mpsc::channel::<Outcome>();
            // On a failed spawn the closure (and with it the job receiver) is dropped, so
            // `submit`'s sends fail and the app falls back to writing inline.
            let _ = std::thread::Builder::new().name("wordcraft-autosave".into()).spawn(move || {
                while let Ok(save) = jobs.recv() {
                    let outcome = write_prepared(&save).map_err(|e| e.to_string());
                    if events.send((save.path, outcome)).is_err() {
                        return;
                    }
                }
            });
            Self { tx, events: rx, busy: false }
        }

        /// Whether a write is in flight; AutoSave waits for its outcome before the next one.
        pub fn is_busy(&self) -> bool {
            self.busy
        }

        /// Hand a prepared save to the worker. Only call this when [`Self::is_busy`] is false.
        pub fn submit(&mut self, save: PreparedSave) {
            if self.tx.send(save).is_ok() {
                self.busy = true;
            }
        }

        /// The outcome of a finished write, if one has ended. Reading it re-arms the worker.
        pub fn poll(&mut self) -> Option<Outcome> {
            match self.events.try_recv() {
                Ok(outcome) => {
                    self.busy = false;
                    Some(outcome)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.busy = false;
                    None
                }
            }
        }

        /// Wait (bounded) for the write in flight, returning its outcome; used when the window
        /// closes so an AutoSave is never cut off mid-write. `None` if nothing finished in time.
        pub fn settle(&mut self, timeout: Duration) -> Option<Outcome> {
            if !self.busy {
                return None;
            }
            let timeout = timeout.max(Duration::from_millis(1));
            let outcome = self.events.recv_timeout(timeout).ok();
            self.busy = false;
            outcome
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use imp::{AutoSaveWorker, Outcome};
