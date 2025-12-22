#!/usr/bin/env bash
# Build a trimmed static FFmpeg matching the decoder pipeline (H.264 decode + minimal demuxers/filters).
# Outputs headers/libs under target/ffmpeg-min by default so FFMPEG_DIR in .cargo/config.toml can point there.
# Env overrides:
#   FFMPEG_VERSION   - FFmpeg release tag without leading "n" (default: 8.0.1)
#   PREFIX           - install prefix (default: target/ffmpeg-min)
#   BUILD_DIR        - work directory for sources/build (default: target/ffmpeg-build)
#   JOBS             - parallel make jobs (default: detected cores)
#   FFMPEG_TOOLCHAIN - set to "msvc" to build with MSVC/Win64 (default: autodetected native toolchain)
#   FFMPEG_ARCH      - arch used when FFMPEG_TOOLCHAIN=msvc (default: x86_64)
#   FFMPEG_TARGET_OS - target os when FFMPEG_TOOLCHAIN=msvc (default: win64)
#   MAKE             - make executable to run (default: make; for MSVC use a GNU make available in PATH)
#   EXTRA_CFLAGS     - override extra cflags (default: -fPIC -O3, or -O2 for MSVC)
#   EXTRA_LDFLAGS    - override extra ldflags (default: -fPIC, or empty for MSVC)
set -euo pipefail

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: missing dependency '$1'" >&2
        exit 1
    fi
}

detect_jobs() {
    if command -v nproc >/dev/null 2>&1; then
        nproc
    elif command -v sysctl >/dev/null 2>&1; then
        sysctl -n hw.ncpu 2>/dev/null || echo 4
    else
        echo 4
    fi
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

require curl
require tar
MAKE_BIN="${MAKE:-make}"
require "${MAKE_BIN}"

FFMPEG_TOOLCHAIN="${FFMPEG_TOOLCHAIN:-}"
FFMPEG_ARCH="${FFMPEG_ARCH:-x86_64}"
FFMPEG_TARGET_OS="${FFMPEG_TARGET_OS:-win64}"

if [[ "${FFMPEG_TOOLCHAIN}" == "msvc" ]]; then
    require cl
fi

FFMPEG_VERSION="${FFMPEG_VERSION:-8.0.1}"
PREFIX="${PREFIX:-${ROOT_DIR}/target/ffmpeg-min}"
BUILD_DIR="${BUILD_DIR:-${ROOT_DIR}/target/ffmpeg-build}"
JOBS="${JOBS:-$(detect_jobs)}"

THREAD_FLAG="--enable-pthreads"
CONFIGURE_TOOLCHAIN=()
EXTRA_CFLAGS_DEFAULT="-fPIC -O3"
EXTRA_LDFLAGS_DEFAULT="-fPIC"
EXTRA_LIBS_DEFAULT=""

if [[ "${FFMPEG_TOOLCHAIN}" == "msvc" ]]; then
    THREAD_FLAG="--enable-w32threads"
    CONFIGURE_TOOLCHAIN=(--toolchain=msvc --arch="${FFMPEG_ARCH}" --target-os="${FFMPEG_TARGET_OS}")
    EXTRA_CFLAGS_DEFAULT="-O2"
    EXTRA_LDFLAGS_DEFAULT=""
    EXTRA_LIBS_DEFAULT="-lbcrypt"
fi

EXTRA_CFLAGS="${EXTRA_CFLAGS:-${EXTRA_CFLAGS_DEFAULT}}"
EXTRA_LDFLAGS="${EXTRA_LDFLAGS:-${EXTRA_LDFLAGS_DEFAULT}}"
EXTRA_LIBS="${EXTRA_LIBS:-${EXTRA_LIBS_DEFAULT}}"

TARBALL="${BUILD_DIR}/ffmpeg-${FFMPEG_VERSION}.tar.gz"
SOURCE_DIR="${BUILD_DIR}/FFmpeg-n${FFMPEG_VERSION}"

mkdir -p "${BUILD_DIR}"
cd "${BUILD_DIR}"

echo "==> Fetching FFmpeg ${FFMPEG_VERSION}"
if [ -f "${TARBALL}" ] && ! file "${TARBALL}" | grep -q "gzip compressed data"; then
    echo "warning: ${TARBALL} is not an .tar.gz; re-downloading" >&2
    rm -f "${TARBALL}"
fi
if [ ! -f "${TARBALL}" ]; then
    curl -fL --retry 3 --retry-delay 1 --connect-timeout 15 \
        "https://github.com/FFmpeg/FFmpeg/archive/refs/tags/n${FFMPEG_VERSION}.tar.gz" \
        -o "${TARBALL}"
fi

rm -rf "${SOURCE_DIR}"
tar -xzf "${TARBALL}"

echo "==> Configuring (prefix=${PREFIX})"
rm -rf "${PREFIX}"
mkdir -p "${PREFIX}"
pushd "${SOURCE_DIR}" >/dev/null

CONFIGURE_ARGS=(
    --prefix="${PREFIX}"
    --pkg-config-flags="--static"
    --enable-static
    --disable-shared
    --enable-pic
    --enable-small
    --disable-programs
    --disable-doc
    --disable-debug
    --disable-network
    --disable-autodetect
    --disable-everything
    --enable-protocol=file
    --enable-demuxer=mov
    --enable-demuxer=matroska
    --enable-demuxer=mpegts
    --enable-parser=h264
    --enable-decoder=h264
    --enable-swresample
    --enable-swscale
    --enable-avcodec
    --enable-avformat
    --enable-avfilter
    --enable-avutil
    --enable-filter=buffer
    --enable-filter=buffersink
    --enable-filter=format
    --enable-filter=scale
    "${THREAD_FLAG}"
    --extra-cflags="${EXTRA_CFLAGS}"
)

if [[ -n "${EXTRA_LDFLAGS}" ]]; then
    CONFIGURE_ARGS+=(--extra-ldflags="${EXTRA_LDFLAGS}")
fi

if [[ -n "${EXTRA_LIBS}" ]]; then
    CONFIGURE_ARGS+=(--extra-libs="${EXTRA_LIBS}")
fi

if [[ ${#CONFIGURE_TOOLCHAIN[@]} -gt 0 ]]; then
    CONFIGURE_ARGS+=("${CONFIGURE_TOOLCHAIN[@]}")
fi

./configure "${CONFIGURE_ARGS[@]}"

echo "==> Building (jobs=${JOBS})"
"${MAKE_BIN}" -j "${JOBS}"
echo "==> Installing to ${PREFIX}"
"${MAKE_BIN}" install
popd >/dev/null

cat <<EOF
Done.
Set env (already in .cargo/config.toml by default):
  FFMPEG_DIR=${PREFIX}
  PKG_CONFIG_PATH=${PREFIX}/lib/pkgconfig:\${PKG_CONFIG_PATH:-}
Then build with:
  cargo build -p subtitle-fast-decoder
EOF
