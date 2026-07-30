//! The PlayerEngine — owns a libmpv instance on a dedicated thread.
//!
//! Architecture:
//!
//!   UI thread ──[Cmd]──▶  Engine thread ──[mpv_*()]──▶ libmpv
//!                          │
//!                          └──[EngineEvent]──▶ UI thread
//!
//! The engine thread runs a single loop:
//!   1. Poll the Cmd channel (non-blocking) — apply any new commands.
//!   2. Call `mpv_wait_event(timeout=0.05)` — drain any pending libmpv events.
//!   3. Translate events to `EngineEvent`s and publish to the bus.
//!   4. Sleep briefly if nothing happened (to avoid busy-looping).
//!
//! The engine owns the `MpvHandle`. When the loop exits, the handle is dropped
//! and libmpv is torn down.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use tracing::{debug, error, info, warn};

use mpv_bindings::event::{Event as MpvEvent, EventId, LogLevel};
use mpv_bindings::handle::Builder as MpvBuilder;
use mpv_bindings::property::{Format, Property};
use mpv_bindings::MpvHandle;

use crate::cmd::{build_loadfile, build_seek, Cmd, LoopMode, MarkerExportFormat};
use crate::error::{CoreError, CoreResult};
use crate::event::{EngineEvent, EngineEventBus, EngineEventSender, EndReason};
use crate::options::EngineOptions;
use crate::state::{PlaybackState, Track};

/// Tags for observed properties. We use these to look up which property
/// changed when we receive a PropertyChange event.
const PROP_TIME_POS: EventId = 1;
const PROP_DURATION: EventId = 2;
const PROP_PAUSE: EventId = 3;
const PROP_VOLUME: EventId = 4;
const PROP_MUTE: EventId = 5;
const PROP_PATH: EventId = 6;
const PROP_MEDIA_TITLE: EventId = 7;
const PROP_SPEED: EventId = 8;
const PROP_EOF_REACHED: EventId = 9;
const PROP_TRACK_LIST_COUNT: EventId = 10;
const PROP_AB_LOOP_A: EventId = 11;
const PROP_AB_LOOP_B: EventId = 12;
const PROP_AID: EventId = 13;
const PROP_SID: EventId = 14;
const PROP_SUB_VISIBILITY: EventId = 15;

/// The engine. Construct with `PlayerEngine::new()`, then `start()`, then
/// issue commands via `send()`. Consume events via `take_event_receiver()`.
pub struct PlayerEngine {
    /// Options captured at construction. The engine thread reads these once.
    options: EngineOptions,
    /// Channel for commands from UI to engine.
    cmd_tx: Sender<Cmd>,
    /// Receiver, consumed by `start()`.
    cmd_rx: Option<Receiver<Cmd>>,
    /// Sender for events from engine to UI. The engine thread writes to this.
    event_tx: EngineEventSender,
    /// Receiver, handed out once via `take_event_receiver()`.
    event_rx: Option<Receiver<EngineEvent>>,
    /// Latest state snapshot, shared with subscribers.
    state: Arc<Mutex<PlaybackState>>,
    /// The engine thread handle (None until start()).
    thread: Option<JoinHandle<()>>,
    /// Wakeup handle so the engine can break out of mpv_wait_event on shutdown.
    /// We hold an Arc<MpvHandle> separately so the main thread can call wakeup().
    handle: Option<Arc<MpvHandle>>,
}

impl PlayerEngine {
    /// Construct a new engine. Does NOT start the thread — call `start()` next.
    pub fn new(options: EngineOptions) -> CoreResult<Self> {
        let (cmd_tx, cmd_rx) = bounded::<Cmd>(64);
        let (event_tx, event_rx) = bounded::<EngineEvent>(256);
        let state = Arc::new(Mutex::new(PlaybackState::default()));
        Ok(Self {
            options,
            cmd_tx,
            cmd_rx: Some(cmd_rx),
            event_tx,
            event_rx: Some(event_rx),
            state,
            thread: None,
            handle: None,
        })
    }

    /// Spawn the engine thread. Returns `Ok` once the engine is ready, or
    /// `Err` if libmpv failed to initialize. Bounds the caller's wait to the
    /// libmpv init time — never longer — and surfaces any init failure as an
    /// `Err`.
    ///
    /// Uses a one-shot channel for the init handshake: the engine thread
    /// publishes `Ok(handle)` or `Err(message)` exactly once when its init
    /// work completes, so `start()` blocks for exactly as long as libmpv
    /// takes to come up.
    pub fn start(&mut self) -> CoreResult<()> {
        if self.thread.is_some() {
            return Err(CoreError::EngineNotRunning);
        }
        let cmd_rx = self.cmd_rx.take().ok_or(CoreError::EngineNotRunning)?;
        let options = self.options.clone();
        let event_tx = self.event_tx.clone();
        let state = self.state.clone();

        let (init_tx, init_rx) = bounded::<Result<Arc<MpvHandle>, String>>(1);

        let thread = thread::Builder::new()
            .name("ferret-engine".into())
            .spawn(move || {
                engine_main(options, cmd_rx, event_tx, state, init_tx);
            })
            .map_err(|e| CoreError::Io(std::io::Error::new(e.kind(), e.to_string())))?;

        let init_result = init_rx
            .recv()
            .map_err(|_| CoreError::EngineStopped)?;

        match init_result {
            Ok(handle) => {
                self.handle = Some(handle);
                self.thread = Some(thread);
                Ok(())
            }
            Err(msg) => {
                // Reap the engine thread to release the JoinHandle. The error
                // event has already been published on the event channel.
                let _ = thread.join();
                Err(CoreError::Other(msg))
            }
        }
    }

    /// Send a command to the engine. Non-blocking; errors only if the channel
    /// is full or the engine has exited.
    pub fn send(&self, cmd: Cmd) -> CoreResult<()> {
        self.cmd_tx.send(cmd).map_err(|_| CoreError::EngineStopped)
    }

    /// Take the event receiver (one-shot; only one consumer is supported).
    pub fn take_event_receiver(&mut self) -> Option<Receiver<EngineEvent>> {
        self.event_rx.take()
    }

    /// Get a clone of the event sender. Single-receiver contract: with
    /// crossbeam bounded channels, the sole consumer obtains the receiver via
    /// `take_event_receiver()`. This accessor exposes the sender for internal
    /// tooling that needs to publish into the same channel.
    #[deprecated(note = "use take_event_receiver() instead")]
    pub fn subscribe(&self) -> EngineEventSender {
        self.event_tx.clone()
    }

    /// Get a clone of the event bus (sender + shared state snapshot).
    pub fn event_bus(&self) -> EngineEventBus {
        EngineEventBus::new(self.event_tx.clone(), self.state.clone())
    }

    /// Snapshot the current playback state.
    pub fn state(&self) -> PlaybackState {
        self.state.lock().clone()
    }

    /// Wake up the engine's `mpv_wait_event` call (used for shutdown).
    pub fn wakeup(&self) {
        if let Some(h) = &self.handle {
            h.wakeup();
        }
    }

    /// Shutdown the engine and wait for the thread to exit.
    pub fn shutdown(&mut self) -> CoreResult<()> {
        if let Some(h) = &self.handle {
            h.wakeup();
        }
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.handle = None;
        Ok(())
    }
}

impl Drop for PlayerEngine {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

// ---- Engine main loop ----

fn engine_main(
    options: EngineOptions,
    cmd_rx: Receiver<Cmd>,
    event_tx: EngineEventSender,
    state: Arc<Mutex<PlaybackState>>,
    init_tx: Sender<Result<Arc<MpvHandle>, String>>,
) {
    let bus = EngineEventBus::new(event_tx.clone(), state.clone());

    // Build the libmpv instance.
    let mut builder = MpvBuilder::new().log_level(parse_log_level(&options.log_level));
    for (k, v) in options.to_mpv_options() {
        builder = builder.option(k, v);
    }
    let mpv = match builder.build() {
        Ok(h) => h,
        Err(e) => {
            let msg = format!("libmpv init: {e}");
            error!("{msg}");
            bus.send(EngineEvent::Error { message: msg.clone() });
            bus.send(EngineEvent::Shutdown);
            let _ = init_tx.send(Err(msg));
            return;
        }
    };

    // Observe properties we care about.
    let observed = [
        (PROP_TIME_POS, "time-pos", Format::Double),
        (PROP_DURATION, "duration", Format::Double),
        (PROP_PAUSE, "pause", Format::Flag),
        (PROP_VOLUME, "volume", Format::Double),
        (PROP_MUTE, "mute", Format::Flag),
        (PROP_PATH, "path", Format::String),
        (PROP_MEDIA_TITLE, "media-title", Format::String),
        (PROP_SPEED, "speed", Format::Double),
        (PROP_EOF_REACHED, "eof-reached", Format::Flag),
        // Track list size — fires when a file loads / unloads, prompting us
        // to enumerate audio tracks via `track-list/N/...` properties.
        (PROP_TRACK_LIST_COUNT, "track-list/count", Format::Int64),
        // A/B loop markers — observed so we keep state in sync if mpv itself
        // changes them (e.g. via a future scripting feature).
        (PROP_AB_LOOP_A, "ab-loop-a", Format::Double),
        (PROP_AB_LOOP_B, "ab-loop-b", Format::Double),
        // Current audio track id (mpv `aid`). Can be "auto" or a number;
        // observe as String so we can handle "auto" cleanly.
        (PROP_AID, "aid", Format::String),
        // Current subtitle track id (mpv `sid`). Same semantics as `aid`.
        (PROP_SID, "sid", Format::String),
        // Subtitle visibility — when false, subtitles are hidden even if a
        // track is selected. (mpv `sub-visibility`.)
        (PROP_SUB_VISIBILITY, "sub-visibility", Format::Flag),
    ];
    for (tag, name, fmt) in observed {
        if let Err(e) = mpv.observe_property(tag, name, fmt) {
            warn!("observe_property({name}) failed: {e}");
        }
    }

    // Initialize state from options (loop mode, etc).
    bus.update_state(|s| {
        s.loop_mode = options.loop_mode;
    });

    // Publish the handle to the caller (start()) and signal readiness.
    let mpv_arc = Arc::new(mpv);
    let _ = init_tx.send(Ok(mpv_arc.clone()));

    info!("ferret engine ready");
    bus.send(EngineEvent::Ready);

    // Main loop.
    loop {
        // 1. Drain pending commands (non-blocking). The first Shutdown wins
        //    and tears the engine down; everything else is dispatched in order.
        for cmd in cmd_rx.try_iter() {
            if matches!(cmd, Cmd::Shutdown) {
                info!("engine received shutdown");
                bus.send(EngineEvent::Shutdown);
                return;
            }
            if let Err(e) = apply_cmd(&mpv_arc, &bus, &cmd) {
                warn!("cmd apply failed: {cmd:?} -> {e}");
                bus.send(EngineEvent::Error { message: format!("{e}") });
            }
        }

        // 2. Wait for next mpv event with a short timeout. This keeps the
        //    loop responsive to commands even when no events arrive.
        match mpv_arc.wait_event(0.05) {
            Ok(Some(event)) => handle_mpv_event(&event, &bus, &mpv_arc),
            Ok(None) => {
                // No event this round.
            }
            Err(mpv_bindings::MpvError::Terminated) => {
                info!("libmpv terminated");
                bus.send(EngineEvent::Shutdown);
                return;
            }
            Err(e) => {
                warn!("mpv_wait_event error: {e:?}");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn apply_cmd(mpv: &MpvHandle, bus: &EngineEventBus, cmd: &Cmd) -> CoreResult<()> {
    match cmd {
        Cmd::LoadFile { path, options } => {
            let mpv_cmd = build_loadfile(path, options)?;
            mpv.command(&mpv_cmd)?;
            if options.pause {
                mpv.set_property(&Property::flag("pause", true))?;
            }
            // Update state immediately so UI shows "loading".
            bus.update_state(|s| {
                s.path = Some(path.clone());
                s.paused = options.pause;
                s.time_pos = None;
                s.duration = None;
            });
            bus.send(EngineEvent::StateChanged);
            Ok(())
        }
        Cmd::PlayPause => {
            let now_paused = mpv.get_property_flag("pause").unwrap_or(false);
            mpv.set_property(&Property::flag("pause", !now_paused))?;
            Ok(())
        }
        Cmd::SetPaused(p) => {
            mpv.set_property(&Property::flag("pause", *p))?;
            Ok(())
        }
        Cmd::Seek { target_secs, mode, flags } => {
            let mpv_cmd = build_seek(*target_secs, *mode, *flags)?;
            mpv.command(&mpv_cmd)?;
            Ok(())
        }
        Cmd::SetVolume(v) => {
            let clamped = v.clamp(0.0, 1.0);
            mpv.set_property(&Property::double("volume", (clamped * 100.0) as f64))?;
            if clamped > 0.0 && mpv.get_property_flag("mute").unwrap_or(false) {
                mpv.set_property(&Property::flag("mute", false))?;
            }
            Ok(())
        }
        Cmd::AdjustVolume(delta) => {
            let cur = mpv.get_property_f64("volume").unwrap_or(0.0) / 100.0;
            let new_v = (cur + (*delta as f64)).clamp(0.0, 1.0);
            mpv.set_property(&Property::double("volume", (new_v * 100.0) as f64))?;
            Ok(())
        }
        Cmd::ToggleMute => {
            let m = mpv.get_property_flag("mute").unwrap_or(false);
            mpv.set_property(&Property::flag("mute", !m))?;
            Ok(())
        }
        Cmd::SetMute(m) => {
            mpv.set_property(&Property::flag("mute", *m))?;
            Ok(())
        }
        Cmd::Stop => {
            mpv.command(&mpv_bindings::command::Command::stop())?;
            Ok(())
        }
        Cmd::FrameStep => {
            mpv.set_property(&Property::flag("pause", true))?;
            mpv.command(&mpv_bindings::command::Command::frame_step())?;
            Ok(())
        }
        Cmd::FrameBackStep => {
            mpv.set_property(&Property::flag("pause", true))?;
            mpv.command(&mpv_bindings::command::Command::frame_back_step())?;
            Ok(())
        }
        Cmd::SetWindowId(_) => {
            warn!("SetWindowId ignored (must be set before engine start)");
            Ok(())
        }
        Cmd::ToggleFullscreen => {
            // This is a window-level concern, not an engine concern. The main
            // app should intercept this command before it reaches the engine.
            // If we get here, log and no-op.
            debug!("ToggleFullscreen reached engine — main app should intercept");
            Ok(())
        }

        // ---- Loop / playlist --------------------------------------------

        Cmd::SetLoopMode(mode) => {
            apply_loop_mode(mpv, *mode)?;
            bus.update_state(|s| s.loop_mode = *mode);
            bus.send(EngineEvent::StateChanged);
            Ok(())
        }
        Cmd::PlaylistNext => {
            let cmd = mpv_bindings::command::Command::new()
                .arg("playlist-next")?
                .arg("weak")?;
            mpv.command(&cmd)?;
            Ok(())
        }
        Cmd::PlaylistPrev => {
            let cmd = mpv_bindings::command::Command::new()
                .arg("playlist-prev")?
                .arg("weak")?;
            mpv.command(&cmd)?;
            Ok(())
        }

        // ---- Speed ------------------------------------------------------

        Cmd::SetSpeed(s) => {
            let v = (*s as f64).clamp(0.01, 100.0);
            mpv.set_property(&Property::double("speed", v))?;
            Ok(())
        }

        // ---- Audio track selection -------------------------------------

        Cmd::SetAudioTrack(opt_id) => {
            match opt_id {
                Some(id) => {
                    mpv.set_property(&Property::int("aid", *id))?;
                }
                None => {
                    // "auto" lets mpv pick the default track.
                    mpv.set_property_string("aid", "auto")?;
                }
            }
            Ok(())
        }

        // ---- Subtitle track selection -----------------------------------

        Cmd::SetSubtitleTrack(opt_id) => {
            match opt_id {
                Some(id) => {
                    mpv.set_property(&Property::int("sid", *id))?;
                    // Selecting a track should also make it visible.
                    mpv.set_property(&Property::flag("sub-visibility", true))?;
                }
                None => {
                    // `sid=no` disables subtitles entirely.
                    mpv.set_property_string("sid", "no")?;
                }
            }
            Ok(())
        }
        Cmd::ToggleSubVisibility => {
            let now = mpv.get_property_flag("sub-visibility").unwrap_or(false);
            mpv.set_property(&Property::flag("sub-visibility", !now))?;
            Ok(())
        }
        Cmd::LoadSubtitleFile { path } => {
            // mpv `sub-add <path> [select|auto|cached] [title] [lang]`.
            // We use "select" so the loaded sub becomes active immediately.
            let cmd = mpv_bindings::command::Command::new()
                .arg("sub-add")?
                .arg(path)?
                .arg("select")?;
            mpv.command(&cmd)?;
            // Refresh track list so the new subtitle appears in the dropdown.
            refresh_audio_tracks(mpv, bus);
            Ok(())
        }

        // ---- Video rotation / flip ------------------------------------

        Cmd::SetVideoRotate(deg) => {
            // mpv's `video-rotate` accepts 0/90/180/270. Anything else snaps to 0.
            // `deg` is `&u16` here (we match on `&Cmd`); deref before forwarding
            // so the result is `u16`, not `&u16`.
            const ALLOWED: &[u16] = &[0, 90, 180, 270];
            let deg: u16 = ALLOWED.contains(deg).then_some(*deg).unwrap_or(0);
            mpv.set_property_string("video-rotate", &deg.to_string())?;
            bus.update_state(|s| s.video_rotate = deg);
            bus.send(EngineEvent::StateChanged);
            Ok(())
        }
        Cmd::SetVideoFlipH(enable) => {
            // Toggle the `hflip` video filter. mpv's vf list is comma-separated;
            // we add or remove `hflip` from it.
            apply_vf_toggle(mpv, "hflip", *enable)?;
            bus.update_state(|s| s.video_flip_h = *enable);
            bus.send(EngineEvent::StateChanged);
            Ok(())
        }
        Cmd::SetVideoFlipV(enable) => {
            apply_vf_toggle(mpv, "vflip", *enable)?;
            bus.update_state(|s| s.video_flip_v = *enable);
            bus.send(EngineEvent::StateChanged);
            Ok(())
        }

        // ---- A/B markers -----------------------------------------------

        Cmd::SetMarkerA => {
            let pos = mpv.get_property_f64("time-pos").ok();
            if let Some(t) = pos {
                mpv.set_property(&Property::double("ab-loop-a", t))?;
                bus.update_state(|s| s.marker_a = Some(t));
                bus.send(EngineEvent::StateChanged);
                info!("marker A set at {t:.3}s");
            } else {
                warn!("SetMarkerA: no time-pos available (no file loaded?)");
            }
            Ok(())
        }
        Cmd::SetMarkerB => {
            let pos = mpv.get_property_f64("time-pos").ok();
            if let Some(t) = pos {
                mpv.set_property(&Property::double("ab-loop-b", t))?;
                bus.update_state(|s| s.marker_b = Some(t));
                bus.send(EngineEvent::StateChanged);
                info!("marker B set at {t:.3}s");
            } else {
                warn!("SetMarkerB: no time-pos available (no file loaded?)");
            }
            Ok(())
        }
        Cmd::ClearMarkers => {
            // mpv uses "no" to disable ab-loop-a/b. Setting to 0 doesn't work
            // — we must use the string form.
            mpv.set_property_string("ab-loop-a", "no")?;
            mpv.set_property_string("ab-loop-b", "no")?;
            bus.update_state(|s| {
                s.marker_a = None;
                s.marker_b = None;
                s.marker_loop_enabled = false;
            });
            bus.send(EngineEvent::StateChanged);
            info!("markers cleared");
            Ok(())
        }
        Cmd::ToggleMarkerLoop => {
            // mpv has no separate "ab-loop enable" property — when both
            // ab-loop-a and ab-loop-b are set to non-"no" values, looping
            // happens automatically. So our toggle is:
            //   * If we're "off" → require both markers set; if so, mark as
            //     "on" (looping is already active because the markers are set).
            //     If either marker is missing, surface an error.
            //   * If we're "on" → mark as "off" but keep the markers (so the
            //     user can re-enable without resetting them). We achieve this
            //     by temporarily clearing the markers... but that loses them.
            //
            // Cleaner approach: when toggling OFF, we DON'T clear markers —
            // we just clear `marker_loop_enabled` in our state. mpv will keep
            // looping, so to actually STOP the loop we have to also clear
            // ab-loop-a/b. So toggle OFF = clear markers.
            //
            // Net behavior: toggle ON = require both markers, set flag.
            //                toggle OFF = clear markers + flag.
            let now_enabled = {
                let st = bus.snapshot();
                !st.marker_loop_enabled
            };
            if now_enabled {
                let a = mpv.get_property_string("ab-loop-a").ok().flatten();
                let b = mpv.get_property_string("ab-loop-b").ok().flatten();
                let a_set = a.as_deref().map(|s| s != "no").unwrap_or(false);
                let b_set = b.as_deref().map(|s| s != "no").unwrap_or(false);
                if !a_set || !b_set {
                    bus.send(EngineEvent::Error {
                        message: "Need both A and B markers before enabling A→B loop".into(),
                    });
                    return Ok(());
                }
                // Markers are set — mpv is already looping. Just update our flag.
                bus.update_state(|s| s.marker_loop_enabled = true);
            } else {
                // Toggle OFF — clear the markers to actually stop the loop.
                mpv.set_property_string("ab-loop-a", "no")?;
                mpv.set_property_string("ab-loop-b", "no")?;
                bus.update_state(|s| {
                    s.marker_a = None;
                    s.marker_b = None;
                    s.marker_loop_enabled = false;
                });
            }
            bus.send(EngineEvent::StateChanged);
            info!("marker loop {}", if now_enabled { "ON" } else { "OFF" });
            Ok(())
        }
        Cmd::ExportMarkers { path, format } => {
            let st = bus.snapshot();
            match export_markers_to_file(path, *format, &st) {
                Ok(()) => {
                    info!("markers exported to {path}");
                    Ok(())
                }
                Err(e) => {
                    let msg = format!("marker export failed: {e}");
                    warn!("{msg}");
                    bus.send(EngineEvent::Error { message: msg.clone() });
                    Err(CoreError::Other(msg))
                }
            }
        }
        Cmd::ImportMarkers { path } => {
            match import_markers_from_file(path) {
                Ok((a, b)) => {
                    // Apply A marker if present.
                    if let Some(t) = a {
                        mpv.set_property(&Property::double("ab-loop-a", t))?;
                        bus.update_state(|s| s.marker_a = Some(t));
                    } else {
                        mpv.set_property_string("ab-loop-a", "no")?;
                        bus.update_state(|s| s.marker_a = None);
                    }
                    // Apply B marker if present.
                    if let Some(t) = b {
                        mpv.set_property(&Property::double("ab-loop-b", t))?;
                        bus.update_state(|s| s.marker_b = Some(t));
                    } else {
                        mpv.set_property_string("ab-loop-b", "no")?;
                        bus.update_state(|s| s.marker_b = None);
                    }
                    bus.update_state(|s| s.marker_loop_enabled = a.is_some() && b.is_some());
                    bus.send(EngineEvent::StateChanged);
                    info!("markers imported from {path} (A={a:?}, B={b:?})");
                    Ok(())
                }
                Err(e) => {
                    let msg = format!("marker import failed: {e}");
                    warn!("{msg}");
                    bus.send(EngineEvent::Error { message: msg.clone() });
                    Err(CoreError::Other(msg))
                }
            }
        }

        Cmd::ExportABLoopVideo { .. } => {
            // This command is intercepted by the main app (which reads A/B
            // markers from state and spawns ffmpeg). If it reaches the engine,
            // something went wrong — log and no-op.
            warn!("ExportABLoopVideo reached engine — should be intercepted by main app");
            Ok(())
        }
        Cmd::Shutdown => unreachable!("handled by caller"),
    }
}

fn handle_mpv_event(event: &MpvEvent, bus: &EngineEventBus, mpv: &MpvHandle) {
    match event {
        MpvEvent::StartFile => {
            debug!("mpv: start-file");
            bus.send(EngineEvent::StartFile);
        }
        MpvEvent::FileLoaded => {
            debug!("mpv: file-loaded");
            // Pull metadata immediately so the UI has it.
            let path = mpv.get_property_string("path").ok().flatten().unwrap_or_default();
            let title = mpv.get_property_string("media-title").ok().flatten();
            let duration = mpv.get_property_f64("duration").ok();
            let time_pos = mpv.get_property_f64("time-pos").ok();
            bus.update_state(|s| {
                s.path = Some(path.clone());
                s.title = title.clone();
                s.duration = duration;
                s.time_pos = time_pos;
                s.paused = false; // assume playing unless we hear otherwise
                // Clear A/B markers — they belong to the previous file.
                s.marker_a = None;
                s.marker_b = None;
                s.marker_loop_enabled = false;
            });
            // Refresh audio tracks — track-list/count may not have fired yet.
            refresh_audio_tracks(mpv, bus);
            bus.send(EngineEvent::FileLoaded { path, title });
        }
        MpvEvent::EndFile { reason, error } => {
            info!("mpv: end-file reason={reason:?} error={error:?}");
            // Table-driven enum projection. `EndFileReason` is `#[repr(u8)]`,
            // so the discriminant indexes 1:1 into END_REASON_MAP.
            let r = END_REASON_MAP[(*reason) as usize];
            if let Some(code) = *error {
                let msg = unsafe_libmpv_error_string(code);
                bus.send(EngineEvent::Error { message: format!("end-file: {msg}") });
            }
            bus.update_state(|s| {
                s.time_pos = None;
            });
            bus.send(EngineEvent::EndReached { reason: r });
        }
        MpvEvent::PropertyChange { reply_userdata, name, value } => {
            let (changed, want_track_refresh) =
                apply_property_change(bus, *reply_userdata, name, value);
            if want_track_refresh {
                refresh_audio_tracks(mpv, bus);
            }
            if changed {
                bus.send(EngineEvent::StateChanged);
            }
        }
        MpvEvent::LogMessage { prefix, level, text } => {
            debug!("mpv log [{prefix}/{level}]: {}", text.trim_end());
            bus.send(EngineEvent::Log {
                prefix: prefix.clone(),
                level: level.clone(),
                text: text.clone(),
            });
        }
        MpvEvent::Shutdown => {
            info!("mpv: shutdown");
            bus.send(EngineEvent::Shutdown);
        }
        MpvEvent::Hook { id, name } => {
            debug!("mpv: hook {name} ({id}) — auto-continuing");
            // We don't use hooks yet, but if one fires we must continue it
            // via `hook-ack <id>`. Constructed as a one-arg command.
            if let Ok(cmd) = mpv_bindings::command::Command::new()
                .arg("hook-ack")
                .and_then(|c| c.arg(format!("{id}")))
            {
                let _ = mpv.command(&cmd);
            }
        }
        MpvEvent::Other { event_id } => {
            debug!("mpv: unhandled event_id={event_id}");
        }
    }
}

fn apply_property_change(
    bus: &EngineEventBus,
    tag: EventId,
    name: &str,
    value: &mpv_bindings::event::PropertyValue,
) -> (bool, bool) {
    use mpv_bindings::event::PropertyValue as V;
    let mut changed = true;
    let mut want_track_refresh = false;
    bus.update_state(|s| {
        match (tag, value) {
            (PROP_TIME_POS, V::Double(d)) => s.time_pos = Some(*d),
            (PROP_DURATION, V::Double(d)) => s.duration = Some(*d),
            (PROP_PAUSE, V::Flag(b)) => s.paused = *b,
            (PROP_VOLUME, V::Double(d)) => s.volume = (*d as f32 / 100.0).clamp(0.0, 1.0),
            (PROP_MUTE, V::Flag(b)) => s.muted = *b,
            (PROP_PATH, V::String(s2)) => s.path = Some(s2.clone()),
            (PROP_MEDIA_TITLE, V::String(s2)) => s.title = Some(s2.clone()),
            (PROP_SPEED, V::Double(d)) => s.speed = *d as f32,
            (PROP_EOF_REACHED, V::Flag(_)) => {
                // eof-reached fires before end-file; let end-file drive the state change.
                changed = false;
            }
            // Track list size changed — re-enumerate audio tracks. We don't
            // update state here; refresh_audio_tracks() does that.
            (PROP_TRACK_LIST_COUNT, V::Int64(_)) => {
                want_track_refresh = true;
                changed = false;
            }
            (PROP_AB_LOOP_A, V::Double(d)) => s.marker_a = Some(*d),
            (PROP_AB_LOOP_A, V::String(st)) => {
                // mpv returns "no" when ab-loop-a is unset, or a number string.
                s.marker_a = if st == "no" { None } else { st.parse::<f64>().ok() };
            }
            (PROP_AB_LOOP_B, V::Double(d)) => s.marker_b = Some(*d),
            (PROP_AB_LOOP_B, V::String(st)) => {
                s.marker_b = if st == "no" { None } else { st.parse::<f64>().ok() };
            }
            (PROP_AID, V::String(st)) => {
                // "auto" or a number. None means auto.
                s.current_audio_track = st.parse::<i64>().ok();
            }
            (PROP_AID, V::Int64(id)) => s.current_audio_track = Some(*id),
            (PROP_SID, V::String(st)) => {
                // "no" means no subtitle track selected; otherwise a number.
                s.current_subtitle_track = if st == "no" { None } else { st.parse::<i64>().ok() };
            }
            (PROP_SID, V::Int64(id)) => s.current_subtitle_track = Some(*id),
            (PROP_SUB_VISIBILITY, V::Flag(b)) => s.sub_visibility = *b,
            // Properties can become None when the file unloads.
            (_, V::None) => {
                // Likely time-pos or duration going to None on end-of-file.
                match tag {
                    PROP_TIME_POS => s.time_pos = None,
                    PROP_DURATION => s.duration = None,
                    PROP_AB_LOOP_A => s.marker_a = None,
                    PROP_AB_LOOP_B => s.marker_b = None,
                    PROP_AID => s.current_audio_track = None,
                    PROP_SID => s.current_subtitle_track = None,
                    _ => changed = false,
                }
            }
            _ => {
                changed = false;
            }
        }
    });
    let _ = name;
    (changed, want_track_refresh)
}

/// Enumerate every track (audio + sub) by walking `track-list/N/*`
/// properties.
///
/// mpv exposes the track list as a series of indexed properties. Without
/// the node format (not yet wrapped in `mpv-bindings::property`), we read
/// each field individually. Audio and subtitle tracks are refreshed
/// together because they share the same `track-list/N` namespace.
fn refresh_audio_tracks(mpv: &MpvHandle, bus: &EngineEventBus) {
    let Some(count) = mpv.get_property_i64("track-list/count").ok() else {
        return;
    };
    if count <= 0 {
        bus.update_state(|s| {
            s.audio_tracks.clear();
            s.subtitle_tracks.clear();
            s.current_audio_track = None;
            s.current_subtitle_track = None;
        });
        bus.send(EngineEvent::StateChanged);
        return;
    }

    let (audio, subs): (Vec<Track>, Vec<Track>) = (0..count)
        .filter_map(|i| read_track(mpv, i))
        .partition(|t| t.kind == "audio");

    let cur_audio = audio.iter().find(|t| t.selected).map(|t| t.id);
    let cur_sub = subs.iter().find(|t| t.selected).map(|t| t.id);
    bus.update_state(|s| {
        s.audio_tracks = audio;
        s.subtitle_tracks = subs;
        s.current_audio_track = cur_audio;
        s.current_subtitle_track = cur_sub;
    });
    bus.send(EngineEvent::StateChanged);
}

/// Read one `track-list/N` entry into a `Track`. Returns `None` for video
/// tracks and unreadable entries — audio/sub tracks only.
fn read_track(mpv: &MpvHandle, i: i64) -> Option<Track> {
    let kind = mpv
        .get_property_string(&format!("track-list/{i}/type"))
        .ok()
        .flatten()
        .unwrap_or_default();
    if kind != "audio" && kind != "sub" {
        return None;
    }
    Some(Track {
        id: mpv.get_property_i64(&format!("track-list/{i}/id")).unwrap_or(0),
        kind,
        title: mpv.get_property_string(&format!("track-list/{i}/title")).ok().flatten(),
        lang: mpv.get_property_string(&format!("track-list/{i}/lang")).ok().flatten(),
        selected: mpv.get_property_flag(&format!("track-list/{i}/selected")).unwrap_or(false),
        default: mpv.get_property_flag(&format!("track-list/{i}/default")).unwrap_or(false),
        forced: mpv.get_property_flag(&format!("track-list/{i}/forced")).unwrap_or(false),
    })
}

/// Toggle a video filter on or off by name. mpv's `vf` property is a
/// comma-separated list of filter strings (e.g. "hflip,vflip"). We read the
/// current list, add or remove the named filter, and write it back.
///
/// This is a simple string-based approach — it doesn't handle filter
/// parameters (e.g. `rotate=90`), only bare filter names like `hflip`,
/// `vflip`, `flip`, `mirror`. For rotation we use the `video-rotate`
/// property instead.
fn apply_vf_toggle(mpv: &MpvHandle, filter: &str, enable: bool) -> CoreResult<()> {
    let current = mpv
        .get_property_string("vf")
        .ok()
        .flatten()
        .unwrap_or_default();
    // mpv returns "" when no filters are set, or a comma-separated list.
    let mut filters: Vec<&str> = if current.is_empty() {
        Vec::new()
    } else {
        current.split(',').collect()
    };
    let already_present = filters.iter().any(|f| {
        // Match the filter name (before any '=' if params present).
        f.split('=').next().unwrap_or(f) == filter
    });
    if enable && !already_present {
        filters.push(filter);
    } else if !enable && already_present {
        filters.retain(|f| f.split('=').next().unwrap_or(f) != filter);
    } else {
        // No change needed.
        return Ok(());
    }
    let new_vf = filters.join(",");
    mpv.set_property_string("vf", &new_vf)?;
    Ok(())
}

/// Translate a `LoopMode` into the corresponding mpv property sets.
///
/// Table-driven: each row is `(file_value, playlist_value)`. `loop-file` and
/// `loop-playlist` are independent mpv properties; only one of them carries
/// `"inf"` at a time per our `LoopMode` enum.
fn apply_loop_mode(mpv: &MpvHandle, mode: LoopMode) -> CoreResult<()> {
    const TABLE: [(LoopMode, &str, &str); 3] = [
        (LoopMode::Off, "no", "no"),
        (LoopMode::File, "inf", "no"),
        (LoopMode::Playlist, "no", "inf"),
    ];
    let (_, file_v, list_v) = TABLE
        .iter()
        .copied()
        .find(|(m, _, _)| *m == mode)
        .expect("LoopMode is exhaustive over the TABLE rows");
    mpv.set_property_string("loop-file", file_v)?;
    mpv.set_property_string("loop-playlist", list_v)?;
    Ok(())
}

/// Write A/B markers to a file. Plain text or JSON depending on `format`.
fn export_markers_to_file(
    path: &str,
    format: MarkerExportFormat,
    state: &PlaybackState,
) -> std::io::Result<()> {
    use std::fs;
    use std::io::Write;

    let content = match format {
        MarkerExportFormat::Text => {
            let mut s = String::new();
            if let Some(p) = &state.path {
                s.push_str(&format!("# file: {}\n", p));
            }
            if let Some(d) = state.duration {
                s.push_str(&format!("# duration: {:.3}s\n", d));
            }
            s.push_str(&format!("# speed: {:.2}x\n", state.speed));
            s.push('\n');
            match state.marker_a {
                Some(t) => s.push_str(&format!("A {}\n", format_hms(t))),
                None => s.push_str("A -\n"),
            }
            match state.marker_b {
                Some(t) => s.push_str(&format!("B {}\n", format_hms(t))),
                None => s.push_str("B -\n"),
            }
            if state.marker_loop_enabled {
                s.push_str("LOOP on\n");
            } else {
                s.push_str("LOOP off\n");
            }
            s
        }
        MarkerExportFormat::Json => {
            // Hand-rolled JSON to keep the output stable and avoid pulling
            // in a JSON serializer just for this one feature.
            let mut s = String::new();
            s.push('{');
            s.push_str(&format!(
                "\"file\":{},",
                json_string(state.path.as_deref().unwrap_or(""))
            ));
            s.push_str(&format!(
                "\"duration\":{},",
                json_num(state.duration.unwrap_or(0.0))
            ));
            s.push_str(&format!("\"speed\":{},", json_num(state.speed as f64)));
            s.push_str(&format!(
                "\"a\":{},",
                state.marker_a.map(json_num).unwrap_or_else(|| "null".into())
            ));
            s.push_str(&format!(
                "\"b\":{},",
                state.marker_b.map(json_num).unwrap_or_else(|| "null".into())
            ));
            s.push_str(&format!(
                "\"loop\":{}",
                if state.marker_loop_enabled { "true" } else { "false" }
            ));
            s.push('}');
            s.push('\n');
            s
        }
    };

    // Create parent dirs if needed (e.g. ~/.config/ferret/markers/x.txt).
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let mut f = fs::File::create(path)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// Read a marker file (.txt or .json) and extract the A and B marker
/// positions in seconds. Returns `(Option<a>, Option<b>)` — `None` for
/// either means the marker was absent or invalid.
///
/// Format detection steps down by extension: `.json` tries the JSON parser,
/// and on any parse failure steps down to the plain-text parser. All other
/// extensions go straight to the text parser.
fn import_markers_from_file(path: &str) -> std::io::Result<(Option<f64>, Option<f64>)> {
    use std::fs;
    let content = fs::read_to_string(path)?;
    if path.ends_with(".json") {
        if let Ok((a, b)) = parse_markers_json(&content) {
            return Ok((a, b));
        }
    }
    Ok(parse_markers_text(&content))
}

/// Parse the plain-text marker format written by `export_markers_to_file`:
///
/// ```text
/// # file: /path/to/video.mp4
/// # duration: 180.000s
/// # speed: 1.00x
///
/// A 00:01:23.456
/// B 00:02:45.000
/// LOOP on
/// ```
///
/// Lines starting with `#` are comments. `A`/`B` lines carry an
/// `HH:MM:SS.mmm` (or `MM:SS` or plain seconds) timestamp. Missing or
/// `-` means the marker is absent.
fn parse_markers_text(content: &str) -> (Option<f64>, Option<f64>) {
    let mut a: Option<f64> = None;
    let mut b: Option<f64> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split on first whitespace: "A 00:01:23.456" → ("A", "00:01:23.456").
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("").trim();
        match key {
            "A" => a = parse_timestamp(val),
            "B" => b = parse_timestamp(val),
            _ => {}
        }
    }
    (a, b)
}

/// Parse the JSON marker format written by `export_markers_to_file`. Only
/// extracts the `a` and `b` fields; ignores everything else.
fn parse_markers_json(content: &str) -> std::io::Result<(Option<f64>, Option<f64>)> {
    // Hand-rolled JSON parsing — we only need to find "a" and "b" keys with
    // numeric or null values. This avoids pulling in a JSON parser dep just
    // for this one feature.
    //
    // We look for `"a": <number>` and `"b": <number>` (or `null`).
    let a = find_json_number(content, "\"a\":");
    let b = find_json_number(content, "\"b\":");
    Ok((a, b))
}

/// Find `"<key>": <value>` in a JSON string and return the value as a number.
/// Returns None if the key is absent or the value is `null`.
fn find_json_number(content: &str, key: &str) -> Option<f64> {
    let idx = content.find(key)?;
    let after = &content[idx + key.len()..];
    let after = after.trim_start();
    if after.starts_with("null") {
        return None;
    }
    // Take chars until we hit a comma, brace, or whitespace.
    let num_str: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+' || *c == 'e' || *c == 'E')
        .collect();
    num_str.parse::<f64>().ok()
}

/// Parse a timestamp in `HH:MM:SS.mmm`, `MM:SS`, or plain-seconds form.
/// Returns None if the string is `-` or unparseable.
fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s == "-" {
        return None;
    }
    // Plain seconds (e.g. "83.456").
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    // HH:MM:SS.mmm or MM:SS — weights table-driven by segment count.
    let parts: Vec<&str> = s.split(':').collect();
    let weights: &[f64] = match parts.len() {
        3 => &[3600.0, 60.0, 1.0],
        2 => &[60.0, 1.0],
        _ => return None,
    };
    parts
        .iter()
        .zip(weights.iter())
        .map(|(p, w)| p.parse::<f64>().ok().map(|v| v * w))
        .collect::<Option<Vec<f64>>>()
        .map(|terms| terms.iter().sum())
}

fn json_string(s: &str) -> String {
    // Minimal JSON string escaping — enough for paths and titles.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_num(f: f64) -> String {
    if f.is_finite() {
        format!("{:.3}", f)
    } else {
        "null".into()
    }
}

/// Format seconds as "HH:MM:SS.mmm" — always 3-digit milliseconds for
/// frame-accurate comparison across exports.
fn format_hms(secs: f64) -> String {
    let total_ms = (secs.max(0.0) * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

fn parse_log_level(s: &str) -> LogLevel {
    const TABLE: &[(&[&str], LogLevel)] = &[
        (&["no", "quiet"], LogLevel::Quiet),
        (&["fatal"], LogLevel::Fatal),
        (&["error"], LogLevel::Error),
        (&["warn"], LogLevel::Warn),
        (&["info"], LogLevel::Info),
        (&["status"], LogLevel::Status),
        (&["v", "verbose"], LogLevel::Verbose),
        (&["debug"], LogLevel::Debug),
        (&["trace"], LogLevel::Trace),
    ];
    let needle = s.to_ascii_lowercase();
    TABLE
        .iter()
        .find(|(keys, _)| keys.iter().any(|k| *k == needle))
        .map(|(_, lvl)| *lvl)
        .unwrap_or(LogLevel::Warn)
}

/// Index-mapped projection from `mpv_bindings::event::EndFileReason` to our
/// `EndReason`. `EndFileReason` carries `#[repr(u8)]` and declares variants in
/// the same order as the rows below, so the discriminant indexes 1:1.
const END_REASON_MAP: [EndReason; 6] = [
    EndReason::Eof,      // EndFileReason::Eof      = 0
    EndReason::Stop,     // EndFileReason::Stop     = 1
    EndReason::Quit,     // EndFileReason::Quit     = 2
    EndReason::Error,    // EndFileReason::Error    = 3
    EndReason::Redirect, // EndFileReason::Redirect = 4
    EndReason::Unknown,  // EndFileReason::Unknown  = 5
];

/// Convert a libmpv error code to a human-readable string via libmpv itself.
/// Steps down to a numeric string when libmpv declines to describe the code.
///
/// Goes through the bindgen-generated `sys::mpv_error_string` rather than a
/// hand-rolled extern declaration, so the FFI surface stays in one place.
fn unsafe_libmpv_error_string(code: i32) -> String {
    unsafe {
        let p = mpv_bindings::sys::mpv_error_string(code);
        if p.is_null() {
            return format!("mpv error {code}");
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
