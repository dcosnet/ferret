//! Events emitted by the engine thread.

use std::sync::Arc;

use crossbeam_channel::Sender;
use parking_lot::Mutex;

use crate::state::PlaybackState;

/// Type alias for the channel the engine publishes events on.
pub type EngineEventSender = Sender<EngineEvent>;

/// Shared, cloneable handle to the engine's event channel.
/// Cloning this gives you another sender; the engine doesn't care how many subscribers exist.
#[derive(Clone)]
pub struct EngineEventBus {
    tx: EngineEventSender,
    /// Latest known playback state — readers can peek without round-tripping
    /// through the engine. Updated atomically by the engine thread.
    state: Arc<Mutex<PlaybackState>>,
}

impl EngineEventBus {
    pub fn new(tx: EngineEventSender, state: Arc<Mutex<PlaybackState>>) -> Self {
        Self { tx, state }
    }

    pub fn send(&self, e: EngineEvent) {
        // Ignore send errors: a closed channel just means the UI went away.
        let _ = self.tx.send(e);
    }

    pub fn snapshot(&self) -> PlaybackState {
        self.state.lock().clone()
    }

    pub(crate) fn update_state(&self, f: impl FnOnce(&mut PlaybackState)) {
        let mut g = self.state.lock();
        f(&mut g);
    }
}

/// All events the engine thread can emit to the UI.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Engine finished initializing libmpv and is ready to accept LoadFile.
    Ready,

    /// The current file has started loading (mpv START_FILE).
    StartFile,

    /// The current file has finished loading and is ready to play (mpv FILE_LOADED).
    /// Engine has populated path/duration in the state snapshot.
    FileLoaded { path: String, title: Option<String> },

    /// A property changed. UI should pull a fresh state snapshot.
    StateChanged,

    /// Playback reached end of file.
    EndReached { reason: EndReason },

    /// libmpv reported an error (e.g. codec init failed, decode error).
    Error { message: String },

    /// A log message from libmpv (level >= threshold).
    Log { prefix: String, level: String, text: String },

    /// Engine is shutting down. No more events will follow.
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Error,
    Redirect,
    Quit,
    Unknown,
}
