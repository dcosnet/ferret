#!/usr/bin/env bash
# scripts/setup-libmpv.sh
#
# Extract libmpv + runtime deps into a local prefix without requiring root.
# Idempotent: safe to re-run.
#
# After running, source the generated env.sh:
#   source ./mpv-prefix/env.sh
#
# Then `cargo build --release` will work.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$HERE/.." && pwd)"
PREFIX="$PROJECT_ROOT/mpv-prefix"
TMPDIR="${TMPDIR:-/tmp}/ferret-deps"
mkdir -p "$PREFIX" "$TMPDIR"

echo "==> Prefix: $PREFIX"
echo "==> Temp download dir: $TMPDIR"

# Packages we need. Split into:
#  - libmpv itself (libmpv2, libmpv-dev)
#  - libmpv's runtime deps (everything ldd complained about)
#  - bindgen's runtime deps (libclang, llvm)
PACKAGES=(
    # libmpv core
    libmpv2 libmpv-dev

    # libmpv runtime deps
    libmujs3 liblua5.2-0 libuchardet0 libpipewire-0.3-0t64 libsndio7.0
    libdisplay-info2 libsixel1 libxpresent1 libegl1 libegl-mesa0
    libva-wayland2 libplacebo349 libass9 libva2 libva-drm2 libva-x11-2
    libvdpau1 libxss1 libxv1 libbluray2 libdvdnav4 liblcms2-2 libzimg2
    libwayland-egl1 libwayland-client0 libwayland-cursor0 libwayland-server0
    libxkbcommon0 libgbm1 libdrm2 libxrandr2 libxi6 libgl1 libglx-mesa0
    libglx0 libgl1-mesa-dri libx11-6 libxcb1 libxcb-randr0 libxcb-xfixes0
    libxext6 libpulse0 libasound2t64 libxrender1 libxcursor1 libxinerama1
    libjpeg62-turbo libcdio19 libcdio-paranoia2 libarchive13 librubberband2

    # winit xkbcommon keyboard support (needed at runtime by winit 0.30)
    libxkbcommon-x11-0 libxcb-xkb1

    # mesa software rasterizer + vulkan (step-down when no real GPU is present)
    mesa-vulkan-drivers libgl1-mesa-dri

    # bindgen / build deps
    libclang1-19 libllvm19 libclang-common-19-dev

    # Xvfb (optional, for headless testing)
    xvfb xauth xserver-xorg-core

    # xkb keyboard layout data (needed by xkbcommon at runtime)
    xkb-data
)

echo "==> Downloading ${#PACKAGES[@]} packages..."
cd "$TMPDIR"
for pkg in "${PACKAGES[@]}"; do
    if ! ls "${pkg}"_*.deb 2>/dev/null >/dev/null; then
        apt-get download "$pkg" 2>/dev/null || echo "  (skip $pkg — not available)"
    fi
done

echo "==> Extracting all .deb files into $PREFIX ..."
cd "$PREFIX"
for deb in "$TMPDIR"/*.deb; do
    [ -e "$deb" ] || continue
    dpkg-deb -x "$deb" . 2>/dev/null || echo "  (failed to extract $(basename "$deb"))"
done

# Write a slimmed-down mpv.pc that skips the Requires.private entries.
# The original .pc lists ~50 transitive deps whose .pc files aren't
# installed; we only need the -lmpv link line for dynamic linking.
PC_FILE="$PREFIX/usr/lib/x86_64-linux-gnu/pkgconfig/mpv.pc"
mkdir -p "$(dirname "$PC_FILE")"
cat > "$PC_FILE" <<PC
prefix=$PREFIX/usr
includedir=\${prefix}/include
libdir=\${prefix}/lib/x86_64-linux-gnu

Name: mpv
Description: mpv media player client library (slim pc - runtime deps only)
Version: 2.5.0
Libs: -L\${libdir} -lmpv
Libs.private: -latomic -pthread -lm -lrt
Cflags: -I\${includedir}
PC

# Write env.sh — source this before building / running.
cat > "$PREFIX/env.sh" <<ENV_SH
# Source this before building/running ferret:
#   source $PREFIX/env.sh
export MPV_PREFIX="$PREFIX"
export PKG_CONFIG_PATH="\$MPV_PREFIX/usr/lib/x86_64-linux-gnu/pkgconfig:\${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="\$MPV_PREFIX/usr/lib/x86_64-linux-gnu:\$MPV_PREFIX/lib/x86_64-linux-gnu:\${LD_LIBRARY_PATH:-}"
export C_INCLUDE_PATH="\$MPV_PREFIX/usr/include:\${C_INCLUDE_PATH:-}"
export LIBRARY_PATH="\$MPV_PREFIX/usr/lib/x86_64-linux-gnu:\$MPV_PREFIX/lib/x86_64-linux-gnu:\${LIBRARY_PATH:-}"
export LIBCLANG_PATH="\$MPV_PREFIX/usr/lib/x86_64-linux-gnu"
export CLANG_RESOURCE_DIR="\$MPV_PREFIX/usr/lib/llvm-19/lib/clang/19"
export PATH="\$MPV_PREFIX/usr/bin:\$HOME/.cargo/bin:\${PATH:-}"
ENV_SH

# Verify the libmpv.so.2 dependency closure is complete.
echo "==> Verifying libmpv.so.2 dependency closure..."
export LD_LIBRARY_PATH="$PREFIX/usr/lib/x86_64-linux-gnu:$PREFIX/lib/x86_64-linux-gnu"
MISSING=$(ldd "$PREFIX/usr/lib/x86_64-linux-gnu/libmpv.so.2" 2>/dev/null | grep "not found" || true)
if [ -z "$MISSING" ]; then
    echo "    all deps resolved"
else
    echo "    missing libs:"
    echo "$MISSING"
    echo "    Install the missing packages and re-run this script."
    exit 1
fi

echo
echo "==> Done. Next steps:"
echo "    source $PREFIX/env.sh"
echo "    cargo build --release"
echo "    ./target/release/ferret /path/to/video.mp4"
