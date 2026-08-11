//! Keyboard shortcut → `Cmd` translation table.
//!
//! Table-driven: each row is `(predicate, factory)` as `fn` pointers
//! (zero-cost, `const`-promotable, no captures). Step-down: the first row
//! whose predicate matches wins. `q` and `f` are intercepted by the caller
//! (`FerretApp::handle_keyboard`) before this table is consulted.
//!
//! Keys that depend on UI-derived values (loop mode, speed) read from a
//! `&PlaybackState` snapshot rather than the overlay renderer directly,
//! keeping this module decoupled from `player-ui`.

use player_core::state::PlaybackState;
use player_core::Cmd;
use winit::keyboard::{Key, NamedKey};

/// Translate a winit `Key` into the engine command it should produce, given
/// the current playback state for keys that depend on UI-derived values
/// (loop mode, speed).
///
/// Returns `None` for keys with no binding.
pub fn key_to_cmd(key: &Key, state: &PlaybackState) -> Option<Cmd> {
    use mpv_bindings::command::{SeekFlags, SeekMode};

    type Arm = fn(&Key) -> bool;
    type Make = fn(&PlaybackState) -> Cmd;

    // ---- Predicates: one per key we recognize. ----
    fn is_space(k: &Key) -> bool { matches!(k, Key::Named(NamedKey::Space)) }
    fn is_m(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "m") }
    fn is_arrow_right(k: &Key) -> bool { matches!(k, Key::Named(NamedKey::ArrowRight)) }
    fn is_arrow_left(k: &Key) -> bool { matches!(k, Key::Named(NamedKey::ArrowLeft)) }
    fn is_arrow_up(k: &Key) -> bool { matches!(k, Key::Named(NamedKey::ArrowUp)) }
    fn is_arrow_down(k: &Key) -> bool { matches!(k, Key::Named(NamedKey::ArrowDown)) }
    fn is_dot(k: &Key) -> bool { matches!(k, Key::Character(s) if s == ".") }
    fn is_comma(k: &Key) -> bool { matches!(k, Key::Character(s) if s == ",") }
    fn is_lbracket(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "[") }
    fn is_rbracket(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "]") }
    fn is_backslash(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "\\") }
    fn is_l(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "l" || s == "L") }
    fn is_minus(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "-") }
    fn is_plus(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "=" || s == "+") }
    fn is_n(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "n" || s == "N") }
    fn is_p(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "p" || s == "P") }
    fn is_v(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "v" || s == "V") }
    fn is_r(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "r" || s == "R") }
    fn is_s(k: &Key) -> bool { matches!(k, Key::Character(s) if s == "s" || s == "S") }

    // ---- Factories: produce the Cmd. State-dependent ones read `state`. ----
    fn play_pause(_: &PlaybackState) -> Cmd { Cmd::PlayPause }
    fn toggle_mute(_: &PlaybackState) -> Cmd { Cmd::ToggleMute }
    fn seek_right(_: &PlaybackState) -> Cmd {
        Cmd::Seek { target_secs: 5.0, mode: SeekMode::Relative, flags: SeekFlags::Keyframes }
    }
    fn seek_left(_: &PlaybackState) -> Cmd {
        Cmd::Seek { target_secs: -5.0, mode: SeekMode::Relative, flags: SeekFlags::Keyframes }
    }
    fn vol_up(_: &PlaybackState) -> Cmd { Cmd::AdjustVolume(0.05) }
    fn vol_down(_: &PlaybackState) -> Cmd { Cmd::AdjustVolume(-0.05) }
    fn frame_step(_: &PlaybackState) -> Cmd { Cmd::FrameStep }
    fn frame_back(_: &PlaybackState) -> Cmd { Cmd::FrameBackStep }
    fn marker_a(_: &PlaybackState) -> Cmd { Cmd::SetMarkerA }
    fn marker_b(_: &PlaybackState) -> Cmd { Cmd::SetMarkerB }
    fn clear_markers(_: &PlaybackState) -> Cmd { Cmd::ClearMarkers }
    fn cycle_loop(state: &PlaybackState) -> Cmd {
        Cmd::SetLoopMode(state.loop_mode.cycle())
    }
    fn speed_down(state: &PlaybackState) -> Cmd {
        Cmd::SetSpeed((state.speed - 0.25_f32).max(0.25))
    }
    fn speed_up(state: &PlaybackState) -> Cmd {
        Cmd::SetSpeed((state.speed + 0.25_f32).min(4.0))
    }
    fn next_track(_: &PlaybackState) -> Cmd { Cmd::PlaylistNext }
    fn prev_track(_: &PlaybackState) -> Cmd { Cmd::PlaylistPrev }
    fn toggle_subs(_: &PlaybackState) -> Cmd { Cmd::ToggleSubVisibility }
    fn random_next(_: &PlaybackState) -> Cmd { Cmd::RandomNext }
    fn cycle_random(state: &PlaybackState) -> Cmd {
        Cmd::SetRandomMode(state.random_mode.cycle())
    }

    // ---- Lookup table. Order matters only for `q`/`f` which are
    //      intercepted by the caller; all other keys are mutually exclusive. ----
    const TABLE: &[(Arm, Make)] = &[
        (is_space,       play_pause),
        (is_m,           toggle_mute),
        (is_arrow_right, seek_right),
        (is_arrow_left,  seek_left),
        (is_arrow_up,    vol_up),
        (is_arrow_down,  vol_down),
        (is_dot,         frame_step),
        (is_comma,       frame_back),
        (is_lbracket,    marker_a),
        (is_rbracket,    marker_b),
        (is_backslash,   clear_markers),
        (is_l,           cycle_loop),
        (is_minus,       speed_down),
        (is_plus,        speed_up),
        (is_n,           next_track),
        (is_p,           prev_track),
        (is_v,           toggle_subs),
        (is_r,           random_next),
        (is_s,           cycle_random),
    ];
    TABLE
        .iter()
        .copied()
        .find(|(arm, _)| arm(key))
        .map(|(_, make)| make(state))
}
