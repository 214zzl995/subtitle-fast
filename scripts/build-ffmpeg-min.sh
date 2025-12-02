#!/usr/bin/env bash
# Build a trimmed static FFmpeg matching the decoder pipeline (H.264 decode + minimal demuxers/filters).
# Outputs headers/libs under target/ffmpeg-min by default so FFMPEG_DIR in .cargo/config.toml can point there.
# Env overrides:
#   FFMPEG_VERSION   - FFmpeg release tag without leading "n" (default: 8.0.2)
#   PREFIX           - install prefix (default: target/ffmpeg-min)
#   BUILD_DIR        - work directory for sources/build (default: target/ffmpeg-build)
#   JOBS             - parallel make jobs (default: detected cores)
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
require gzip
require make
require pkg-config

FFMPEG_VERSION="${FFMPEG_VERSION:-8.0.1}"
PREFIX="${PREFIX:-${ROOT_DIR}/target/ffmpeg-min}"
BUILD_DIR="${BUILD_DIR:-${ROOT_DIR}/target/ffmpeg-build}"
JOBS="${JOBS:-$(detect_jobs)}"

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

./configure \
    --prefix="${PREFIX}" \
    --pkg-config-flags="--static" \
    --enable-static \
    --disable-shared \
    --enable-pic \
    --enable-small \
    --disable-programs \
    --disable-doc \
    --disable-debug \
    --disable-network \
    --disable-autodetect \
    --disable-everything \
    --enable-protocol=file \
    --enable-demuxer=mov \
    --enable-demuxer=matroska \
    --enable-demuxer=mpegts \
    --enable-parser=h264 \
    --enable-decoder=h264 \
    --enable-swresample \
    --enable-swscale \
    --enable-avcodec \
    --enable-avformat \
    --enable-avfilter \
    --enable-avutil \
    --enable-filter=buffer \
    --enable-filter=buffersink \
    --enable-filter=format \
    --enable-filter=scale \
    --enable-pthreads \
    --extra-cflags="-fPIC -O3" \
    --extra-ldflags="-fPIC"

echo "==> Building (jobs=${JOBS})"
make -j "${JOBS}"
echo "==> Installing to ${PREFIX}"
make install
popd >/dev/null

cat <<EOF
Done.
Set env (already in .cargo/config.toml by default):
  FFMPEG_DIR=${PREFIX}
  PKG_CONFIG_PATH=${PREFIX}/lib/pkgconfig:\${PKG_CONFIG_PATH:-}
Then build with:
  cargo build -p subtitle-fast-decoder --features backend-ffmpeg,ffmpeg-static
EOF
