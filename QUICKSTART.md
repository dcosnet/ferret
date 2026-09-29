# Quickstart

**Get ferret running in under 5 minutes.**

This guide walks you through installing dependencies, building ferret, and
playing your first video. For full documentation, see [README.md](README.md).

---

## 1. Install Rust

ferret requires Rust 1.75 or later. Install via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version  # should print 1.75.0 or higher
```

If you already have Rust, update to the latest stable:

```bash
rustup update stable
```

---

## 2. Get the source

```bash
git clone http://git.dcos.net/dcosnet/ferret.git
cd ferret
```

---

## 3. Set up libmpv (one-time)

ferret links against libmpv 2.x. The `setup-libmpv.sh` script downloads the
libmpv `.deb` packages and extracts them into a local prefix — **no root
required**.

```bash
./scripts/setup-libmpv.sh
```

This takes about 30 seconds. When it finishes, source the env script:

```bash
source ./mpv-prefix/env.sh
```

You need to source this script in **every terminal** before building or
running ferret (or add it to your `~/.bashrc`).

### What the script installs

- `libmpv2`, `libmpv-dev` — the libmpv shared library and headers
- `libxkbcommon-x11-0`, `libxcb-xkb1`, `xkb-data` — winit keyboard support
- `mesa-vulkan-drivers`, `libgl1-mesa-dri` — GPU drivers (software fallback)
- `libclang1-19`, `libllvm19` — for bindgen (FFI generation)
- `xvfb`, `xauth` — for headless testing (optional)

### Verification

```bash
pkg-config --modversion mpv   # should print "2.5.0" or similar
```

---

## 4. Build

```bash
cargo build --release
```

First build takes about 2 minutes (compiles bindgen + winit + egui + wgpu).
Subsequent builds are incremental.

The binary is at `target/release/ferret`. The build bakes in an rpath to
`mpv-prefix/usr/lib/x86_64-linux-gnu/`, so you don't need
`LD_LIBRARY_PATH` at runtime.

### Build flags

```bash
./scripts/build.sh --debug   # debug build
./scripts/build.sh --clean   # clean + rebuild
./scripts/build.sh --test    # run cargo test
./scripts/build.sh --lint    # clippy + bracket audit + deref audit
./scripts/build.sh --ci      # full CI pass (lint + test + build)
```

---

## 5. Install runtime dependencies

ferret needs one external tool at runtime:

### ffmpeg (for A-B loop video export)

```bash
# Debian/Ubuntu
sudo apt install ffmpeg

# Fedora
sudo dnf install ffmpeg
```

ffmpeg is only needed for **Playback → Export A-B Loop Video...**. If you
don't plan to use that feature, you can skip it.

File dialogs are built into the UI — no zenity, kdialog, or rfd required.

---

## 6. Play a video

```bash
./target/release/ferret /path/to/video.mp4
```

You should see:

1. A dark grey window (1280×720 by default)
2. A menu bar at the top-left: **File Playback Audio* Subtitles Video Help**
   plus a status line (the Audio menu appears once a file with audio tracks
   is loaded). The menu bar and control bar auto-hide after ~3 seconds
   without input — move the mouse to bring them back.
3. The video starts playing immediately

If you launch with no arguments:

```bash
./target/release/ferret
```

You get an empty dark grey window. Click **File → Load File...** to open the
in-UI file browser and pick a file.

---

## 7. Basic controls

| Action | How |
|--------|-----|
| Play / Pause | Space, or click the ▶/⏸ button |
| Seek | Drag the seek bar, or ← / → keys |
| Volume | Drag the slider, or ↑ / ↓ keys |
| Mute | M key, or click the speaker icon |
| Fullscreen | F key, or click the ⛶ button |
| Quit | Q key, or File → Quit |

---

## 8. Try the A/B loop export

This is ferret's signature feature — extract a clip from a video using
A/B markers:

1. Play the video to the start of the segment you want.
2. Press `[` to set marker A.
3. Play to the end of the segment.
4. Press `]` to set marker B.
5. Click **Playback → Export A-B Loop Video...**
6. The in-UI file browser opens — pick where to save the clip.
7. ffmpeg runs in the background and renders the segment.

You'll see red and blue pins on the seek bar marking A and B. An info toast
appears when the export is done.

---

## 9. Load a folder as a playlist

```
File → Load Folder... → pick a directory
```

ferret scans the folder for video files (`.mp4`, `.mkv`, `.webm`, `.avi`,
`.mov`, `.flv`, `.mp3`, `.ogg`, `.wav`, `.flac`, etc.), sorts them
alphabetically, and loads them as a playlist. Use **N** / **P** keys (or
the ⏭ / ⏮ buttons) to move between entries.

To loop the whole playlist: **Playback → Loop → Loop Playlist**.

---

## Troubleshooting

### "failed to locate libmpv via pkg-config"

You forgot to source the env script:

```bash
source ./mpv-prefix/env.sh
```

### "libmpv.so.2: cannot open shared object file"

Same fix — source the env script. Or set `LD_LIBRARY_PATH` manually:

```bash
export LD_LIBRARY_PATH=./mpv-prefix/usr/lib/x86_64-linux-gnu:$LD_LIBRARY_PATH
```

### "ffmpeg not found in PATH"

Install ffmpeg (see step 5). This only affects A-B loop video export.

### Window opens but is transparent / shows desktop

This happens when no video is loaded — the overlay clears to dark grey
(`#1a1a1d`). If you're seeing the desktop instead, make sure you're running
on X11 (Wayland is not yet supported — see [README.md](README.md#roadmap)).

### File dialogs don't appear

File dialogs are built into the ferret UI — no external tools needed. If
the dialog doesn't open, check that the overlay is receiving mouse events
(move the mouse over the window).

---

## Next steps

- Read the full [README.md](README.md) for architecture details and the
  complete keyboard shortcut reference
- Read [BLOG.md](BLOG.md) for the development history and design decisions
- File bugs at <http://git.dcos.net/dcosnet/ferret/issues>

---

## Uninstall

ferret doesn't install anything system-wide. To remove:

```bash
rm -rf /path/to/ferret           # removes source + build + mpv-prefix
```

The `mpv-prefix/` directory contains the extracted libmpv packages — it's
self-contained and safe to delete.
