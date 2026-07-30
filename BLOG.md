# ferret Development Blog

A narrative history of ferret's development, design decisions, and the bugs
that shaped the codebase. This is not a changelog (see git log for that) —
it's the story behind the code.

**Author:** Jeremy Anderson
**Website:** http://git.dcos.net/dcosnet/ferret

---

## Why I built ferret

I watch a lot of video. Documentaries, lectures, archival footage. And I'm
picky about playback quality — I notice frame drops, audio drift, and
smearing on corrupt frames. VLC was fine until around 2018, when it started
stuttering on my hardware. mpv was better, but its UI is minimalist to a
fault — I wanted something between VLC's cluttered chrome and mpv's bare
keyboard-driven interface.

The SMPlayer approach (separate frontend process talking to mpv) always
appealed to me, but SMPlayer itself is a Qt app that feels dated. I wanted
something modern, written in a memory-safe language, with the accuracy-first
philosophy of mpv baked in from the start.

Rust + egui + libmpv seemed like the right combination. ferret is the result.

---

## v0.1 — The MVP (the option-error bug)

The first version was a four-crate workspace:

- `mpv-bindings` — bindgen FFI to libmpv
- `player-core` — headless engine with command/event channels
- `player-ui` — egui + wgpu overlay renderer
- `player-app` — binary, multi-window winit event loop

The architecture was sound: libmpv owns decode + VO + AO, the engine runs on
its own thread, the UI renders in a transparent always-on-top overlay
window. Threading model was thread-per-subsystem with crossbeam channels —
no async, just message passing.

### The msg-level bug

The very first run crashed with `libmpv init: mpv: option error`. The
overlay window appeared with a red error toast and a GNOME "Force Quit?"
dialog — the whole app hung.

It took a C probe to find the culprit: `msg-level=warn` is rejected by
libmpv 2.x with `MPV_ERROR_OPTION_ERROR`. The `msg-level` parser requires
`module=level` form — a bare level string like `warn` was accepted by older
mpv builds but is now an error. The fix was `msg-level=all=warn`.

**Lesson:** Always validate libmpv option strings against the actual library
version. The mpv docs are a reference, not a contract — behavior changes
between major versions.

### The 1-second hang

There was a secondary bug hiding behind the option error. `engine.start()`
polled a shared `Mutex<Option<Arc<MpvHandle>>>` for up to 1 second waiting
for the engine thread to publish its handle. When libmpv init failed, the
engine thread exited without publishing, so the main thread blocked
pointlessly for 1 second while the window was already non-responsive — that's
what triggered GNOME's "Force Quit?" dialog.

The fix was a one-shot channel: the engine thread sends `Ok(handle)` or
`Err(message)` the moment init completes (success or failure). `start()`
returns as soon as it gets the message — never longer than the libmpv init
time.

---

## v0.2 — The winit panic (the outer_size bug)

With the option error fixed, the next run panicked inside winit:

```
thread 'main' panicked at winit-0.30.13/src/platform_impl/linux/x11/window.rs:318:41:
called `Result::unwrap()` on an `Err` value: TryFromIntError(PosOverflow)
```

Line 318 was `dimensions.1.try_into().unwrap()` — winit converting the
window height from `u32` to `u16` (X11's `CreateWindow` protocol uses
`u16` width/height). `PosOverflow` meant the value was bigger than 65535.

The value came from `video.outer_size()`, which queries the window
manager's `_NET_FRAME_EXTENTS` property. After libmpv attached to the
video window via `wid`, Xfwm4 started returning garbage extents. The bogus
extents got `saturating_add`-ed to the inner size, capping at `u32::MAX`,
which then overflowed the `u32 → u16` cast.

### The fix

Two changes:

1. **Use `inner_size()` / `inner_position()` instead of `outer_size()` /
   `outer_position()`.** These query `XGetGeometry` / `XTranslateCoordinates`
   directly, never the WM-supplied frame extents. They're robust against
   the libmpv/WM race.

2. **Hard clamp dimensions to 16384.** Even if a bogus value sneaks through,
   the clamp prevents the overflow panic.

**Lesson:** WM-supplied properties are unreliable. libmpv attaching to a
window changes its properties in ways the WM doesn't always track correctly.
Prefer X11 core protocol queries over EWMH hints when the WM might be
confused.

---

## v0.3 — Feature build-out (loop, speed, A/B markers, subtitles)

With playback stable, I built out the feature set. The design principle
was: every feature should work via both UI (menu/button) and keyboard, and
the UI should never block the engine.

### Loop modes

mpv has two separate properties: `loop-file` (repeat current file) and
`loop-playlist` (repeat entire playlist). I modeled this as a `LoopMode`
enum (`Off` / `File` / `Playlist`) with a `cycle()` method — click the
loop button to cycle through the three states.

### Speed control

Speed was straightforward — `mpv speed` property, observed so the UI
stays in sync. I added both preset buttons (0.5×, 1×, 1.5×, 2×) and a
fine-grained slider (0.25×–4×). The slider uses a local `speed_drag` value
during dragging to avoid fighting with the engine's property-change echo.

### A/B markers

This is where I learned that mpv's `ab-loop` is not a property — it's a
**command**. `mpv_set_property_string("ab-loop", "yes")` returns
`MPV_ERROR_PROPERTY_NOT_FOUND`. The actual semantics:

- `ab-loop-a` and `ab-loop-b` are properties (set to a timestamp or `"no"`)
- When both are set to non-`"no"` values, mpv loops between them automatically
- There's no separate "enable" flag

So `ToggleMarkerLoop` became: if turning on, require both markers set; if
turning off, clear both markers (which actually stops the loop). The
`marker_loop_enabled` flag is purely for UI display.

### Audio + subtitle tracks

I refactored `AudioTrack` into a unified `Track` struct (with `kind: "audio"
| "sub"` and a `forced` flag for subtitle tracks). The engine's
`refresh_audio_tracks` function (now conceptually "refresh all tracks")
enumerates `track-list/N/*` for each track N, splits them into audio and
subtitle vectors, and publishes both in `PlaybackState`.

For subtitle visibility, I observed `sub-visibility` (a flag property) so
the "Show Subtitles" checkmark stays in sync. Selecting a track via
`SetSubtitleTrack` also forces visibility on — selecting a track you can't
see is a UX dead end.

---

## v0.4 — The menu that wouldn't lay out horizontally

This was the most embarrassing bug in the project. I added a File menu using
`egui::menu::menu_button`, and it worked — but the menu buttons stacked
vertically instead of horizontally. I wanted a horizontal menu bar
(File | Playback | Audio | Subtitles across the top), not a vertical stack.

### First attempt (wrong)

I wrapped everything in `ui.vertical()`, which made it... still vertical.

### Second attempt (wrong)

I removed the `ui.vertical()` wrapper. Still vertical. The problem wasn't
the wrapper — it was that `egui::Area::show()` gives you a `ui` with
`Layout::TopDown` (vertical) by default. Every `ui.menu_button()` call
stacked on top of the previous one.

### Final fix

Two changes:

1. **Use `egui::Frame` for the background.** `Frame::none().fill(...).show(
   ui, |ui| { ... })` auto-sizes to its content and paints the fill behind
   the widgets. No manual painting needed.

2. **Wrap menu buttons in `ui.horizontal()`.** This overrides the Area's
   default vertical layout and lays out buttons left-to-right.

**Lesson:** egui's layout defaults are not always what you want. `Area` is
vertical by default; if you want horizontal, you must explicitly ask for it.
And don't try to paint backgrounds manually before widgets exist — use
`Frame`, which handles sizing for you.

---

## v0.5 — Overlay covering file dialogs

After the menu layout was fixed, the user reported that file dialogs were
covered by the overlay window. The overlay was created with
`WindowLevel::AlwaysOnTop`, which meant it stayed above everything —
including the zenity file picker.

### The half-fix (v0.5)

I added a `dialogs_open` counter and an `update_overlay_window_level()`
method that drops the overlay to `WindowLevel::Normal` while any dialog is
open, then restores `AlwaysOnTop` when all dialogs close.

But I only called `update_overlay_window_level()` when dialogs *close*
(inside `poll_dialog_results`). I forgot to call it when dialogs *open*. So
the overlay stayed `AlwaysOnTop` the entire time a dialog was open.

### The full fix (v0.6)

Added the call right before spawning the worker threads. The order is
critical — `update_overlay_window_level()` must run BEFORE
`spawn_dialog_worker()` because the dialog opens immediately in the worker
thread. Now the overlay drops to `Normal` before the zenity process starts,
so the picker appears on top.

---

## v0.6 — Polish and documentation

This version focused on:

### Dark grey background when no video loaded

v0.5 cleared the overlay to transparent. With no video loaded, this meant
the overlay showed the desktop through the window — confusing and ugly.
v0.6 makes the clear color conditional: `#1a1a1d` (matching the VLC dark
theme bg) when `state.path.is_none()`, transparent when a file is loaded so
libmpv's video shows through.

### A-B loop video export

The original "Export Markers" feature exported marker *metadata* (A/B
timestamps to a .txt or .json file). The user clarified: they wanted to
export the actual *video segment* between A and B. Completely different
feature.

I added "Export A-B Loop Video...", which reads A/B markers + file path
from engine state, opens a save dialog, then spawns ffmpeg to render the
clip. ffmpeg must be in PATH. The command re-encodes video (libx264, CRF 18)
for frame accuracy and maximum compatibility.

### Documentation

v0.6 adds proper documentation: README.md, QUICKSTART.md, and this BLOG.md.

---

## v1.0 — Production readiness

The jump to v1.0 was driven by a quality assurance pass and several rounds
of bug fixing. Here's what happened:

### The QA audit

A senior QA team audited the codebase against PEP 868, POSIX, SEI CERT, and
MISRA coding standards. The main findings:

1. **Nested-if / branch-heavy code** → refactored to `const TABLE` lookups
   (log-level parser, loop-mode projector, video-rotate clamp, EndFileReason
   mapper, keyboard dispatcher).
2. **`for`/`while` loops** → replaced with iterators (`try_iter`,
   `filter_map().partition()`, `[-1.0, 0.0, 1.0].iter().for_each()`).
3. **Dead code** → deleted `input.rs` (fully shadowed by `main.rs`'s
   keyboard handler), `_silence_warn` hack, `let _ = rect; // suppress unused`.
4. **Decisive-language cleanup** → purged "restored/brought back/falls
   back/kept for compatibility/non-breaking/previously" from all code
   comments and docs. Everything rewritten as decisive present-tense
   design statements.
5. **Step-down logic** → the file dialog backends (zenity/kdialog/rfd)
   refactored into `or_else` chains following the Unix step-down philosophy.

### Compile errors along the way

Three compile errors surfaced during the QA refactor, each teaching a lesson:

1. **`&u16` vs `u16`** — when matching on `&Cmd`, every captured field is a
   reference. `SetVideoRotate(deg)` binds `deg` as `&u16`, so `.then_some(deg)`
   produces `Option<&u16>`, and `.unwrap_or(0)` fails. Fixed with `*deg`.

2. **Stray paren** — a `)` left at the end of a format string in
   `extract_x11_xid`. The bracket-balance audit (`scripts/audit_brackets.py`)
   now catches this in milliseconds.

3. **`wgpu::SurfaceError::Suboptimal`** — doesn't exist in wgpu 22 (folded
   into `Outdated`). The error recovery path was updated.

### Complete hotkey coverage

Every keyboard shortcut now has a menu entry and/or a control-bar button.
Menu items display their shortcut in parentheses (e.g. `Play / Pause (Space)`).
The keyboard dispatch table was extracted to `keymap.rs`, decoupled from
`player-ui` by taking `&PlaybackState` instead of `&Option<OverlayRenderer>`.

---

## v1.0.1–v1.0.4 — The resize/move "mirrored desktop" saga

After v1.0, the user reported that dragging or resizing the window caused
the entire UI to corrupt with "mirrored desktop" artifacts — the window
showed stale framebuffer content from behind it.

This took four rounds to fix completely. Each round revealed a deeper root
cause.

### Round 1: Boolean flag (v1.0.1)

First attempt: a `surface_just_reconfigured` boolean flag that forced the
overlay to clear opaque for one frame after every surface reconfiguration.

**Why it failed:** during a window *move* (not resize), the overlay's size
doesn't change, so `resize()` never fires and the flag never gets set. The
overlay stays transparent throughout the drag.

### Round 2: Timestamp grace period (v1.0.2)

Second attempt: replaced the boolean with `force_opaque_until: Option<Instant>`
— a timestamp set to `now + 150ms` on every `Moved` or `Resized` event.

**Why it failed:** the render was being skipped (surface `Outdated`), so the
old transparent frame stayed visible even with the grace period set.

### Round 3: X11 background pixel (v1.0.3)

Third attempt — I found the actual root cause. winit creates windows with
`background_pixel = None`. This means the X server does NOT fill the window
on resize — it shows whatever is in the framebuffer (stale content, GPU
garbage). That's the "mirror."

**Fix:** `XSetWindowBackground` via FFI, setting the video window's
background to `#141416` (dark grey, matching the VLC theme). Called in
`setup()` before libmpv attaches via `wid`. Now during a resize, the X
server fills the video window with dark grey instead of garbage.

Also fixed: overlay set to `WindowLevel::AlwaysOnTop` (it had regressed to
`Normal`), and `resize()` made a no-op when the size hasn't changed (to
avoid destroying the framebuffer on every `Moved` event).

### Round 4: wgpu presentation fixes (v1.0.4)

The runtime logs revealed the final piece:

```
WARN Unrecognized present mode 1000361000
WARN EGL says it can present to the window but not natively
WARN Detected a linear (sRGBA aware) framebuffer Bgra8UnormSrgb
```

**Fixes:**
1. **Reconfigure + retry on `Outdated`** (revert Round 2's skip-on-Outdated).
   The render path now ALWAYS produces a fresh frame — no stale transparent
   frames left visible.
2. **Force `PresentMode::Fifo`** — Mailbox was broken on this X11/EGL setup.
3. **Force `CompositeAlphaMode::Auto`** — PreMultiplied caused compositing
   artifacts on Xfwm4.
4. **Prefer non-sRGB surface format** — egui warned about sRGB framebuffers.
5. **Handle the overlay's own `Moved` event** — when the overlay moves
   (because we called `set_outer_position()`), its surface can become stale.

**The lesson:** all previous fixes managed the overlay's transparency
*timing* but assumed the render would actually *succeed*. When the surface
was `Outdated`, the render was skipped and the old transparent frame stayed.
The fix was to always reconfigure + retry, ensuring a fresh frame is always
painted.

---

## v1.0.5 — In-UI file browser (eliminating the pop-under)

The final issue: the overlay's `AlwaysOnTop` setting meant external file
dialogs (zenity/kdialog/rfd) popped *under* the overlay — invisible to the
user. Dropping to `Normal` during dialogs was a half-fix that reintroduced
the resize/move bugs.

**The fix:** replace all external file dialogs with an in-UI file browser
drawn inside the egui overlay. This eliminates the z-order conflict entirely
— the browser IS part of the overlay, so it's always visible and always on
top of the video window.

### Implementation

New module `crates/player-ui/src/file_dialog.rs` — a complete file browser:
- Directory listing with sorted entries (dirs first, then files)
- Extension filtering per dialog kind (media, subtitle, marker, video export)
- Single-select, multi-select (Load Playlist), and save modes (filename input)
- Path bar with Up / Home navigation
- Modal overlay: dims the background, blocks interaction with controls behind it
- Seven dialog kinds: LoadFile, LoadFolder, LoadPlaylist, SaveMarkers,
  LoadSubtitle, ImportMarkers, ExportVideo

Menu buttons now open the in-UI dialog directly instead of sending magic
Cmd strings. The `dialogs.rs` module (zenity/kdialog/rfd) was deleted
entirely, and the `rfd` dependency was removed from `Cargo.toml`.

### The borrow-after-move bug

First compile of the in-UI browser had a classic Rust ownership bug: I
cleared `self.file_dialog = None` before calling
`handle_file_dialog_result()`, but that function tried to read the kind from
`self.file_dialog.as_ref()` — which was already `None`. No file ever loaded.

**Fix:** capture `dialog.kind.clone()` before clearing the dialog, and pass
it as a parameter to `handle_file_dialog_result(res, kind)`.

### The channel deadlock

The `LoadFolder` and `LoadPlaylist` handlers sent dozens of `Cmd::LoadFile`
commands in a tight loop using blocking `send()`. The engine's command
channel is bounded at 64 slots — if the engine was busy loading the first
file, the channel filled up and `send()` blocked, freezing the UI.

**Fix:** switched to `try_send()` (non-blocking). If the channel is full,
remaining files are skipped. The info toast shows `"Loaded 3/47 files"`.

---

## Design decisions worth recording

### Why thread-per-subsystem, not async?

The engine owns libmpv on a dedicated thread. Commands flow UI → engine via
a `crossbeam` channel; events flow engine → UI via another channel + shared
`Arc<Mutex<PlaybackState>>`. No async runtime, no futures, no tokio.

Reasons:

1. **libmpv is not async-friendly.** It's a C library with a blocking event
   loop. Wrapping it in async would add complexity without benefit.
2. **Debuggability.** A thread-per-subsystem model has a simple call stack
   — you can `gdb attach` and see exactly what each thread is doing. Async
   stacks are spread across executors and harder to reason about.
3. **Backpressure.** Bounded channels give natural backpressure. If the UI
   can't keep up with events, the engine blocks on send rather than dropping
   or queuing unboundedly.

### Why a separate overlay window?

The overlay is a transparent, borderless, `AlwaysOnTop` window sized to
exactly cover the video window. egui + wgpu renders controls into it. The
video window is handed to libmpv via `wid` — libmpv renders directly into
it using Vulkan/OpenGL.

The two-window approach has one significant downside: mouse events don't
pass through the overlay to the video window. Clicking outside the control
bar does nothing (doesn't toggle pause). This is a known limitation;
planned fix is the libmpv render-context API (single-window compositing).

### Why in-UI file dialogs instead of zenity/kdialog/rfd?

The overlay is `AlwaysOnTop`, which means external file dialogs (separate
OS windows) pop under the overlay — invisible to the user. Drawing the file
browser inside the egui overlay eliminates this z-order conflict entirely.

The downside is that the in-UI browser doesn't have all the features of a
native file dialog (bookmarks, recent files, search). But it's always
visible, always on top of the video window, and has no external dependencies.

### Why GPL-2.0?

ferret links against libmpv, which is LGPL-2.1+ (or GPL-2+ at your option).
Dynamic linking keeps ferret's license compatible with either. I chose
GPL-2.0-or-later because:

1. **It's a media player.** The media player ecosystem has a tradition of
   copyleft (VLC is GPL-2.0+, mpv is GPL-2.0+).
2. **It prevents proprietary forks.** If someone builds on ferret, they have
   to share their changes.
3. **"Or later" clause** allows future compatibility with GPL-3.0 if needed.

### Why hard-cap volume at 100%?

VLC's software amplification (going above 100%) conflicts with PipeWire's
logarithmic volume curves. The result is audio that clips or distorts at
high volumes. By hard-capping at 100% and mapping the slider 1:1 to the
system sink, ferret avoids the clash entirely. If you need louder, turn up
your system volume — that's what it's there for.

### Why force `PresentMode::Fifo`?

During the resize/move debugging, the runtime logs showed "Unrecognized
present mode 1000361000" — wgpu was trying to use Mailbox, which the
X11/EGL driver doesn't support. This caused broken presentation during
window operations. `Fifo` (vsync) is the WebGPU-required mode and works
everywhere. The slight latency cost is worth the stability.

---

## What's next

The roadmap is in the README. The big one is Wayland support via
`mpv_render_context`, which would eliminate the X11 `wid` dependency and
unblock macOS/Windows. After that: config file, playlist UI, MPRIS.

ferret is a hobby project — I work on it when I have time. Patches are
welcome at <http://git.dcos.net/dcosnet/ferret>.

— Jeremy Anderson, 2026
