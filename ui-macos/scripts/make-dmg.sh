#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROJECT="$ROOT/ui-macos/subtitle-fast.xcodeproj"
SCHEME="subtitle-fast"
DERIVED_DATA="$ROOT/ui-macos/DerivedData"
DIST_DIR="$ROOT/ui-macos/dist"
BACKGROUND_IMAGE="${BACKGROUND_IMAGE:-$ROOT/ui-macos/scripts/dmg-background.png}"
VOLNAME_BASE="SubtitleFast"
KEEP_STAGE=0
VARIANTS=()
FFMPEG_BACKEND=1
NO_FFMPEG_FEATURES="subtitle-fast/detector-vision,subtitle-fast/detector-parallel,subtitle-fast/ocr-vision,subtitle-fast-decoder/backend-videotoolbox"

usage() {
    cat <<'EOF'
Usage: make-dmg.sh [options]

Options:
  --arch [arm64|x86_64|universal]  Build DMG for the specified target (repeatable).
  --all                            Build DMGs for both arm64 and x86_64.
  --universal                      Build a universal (arm64 + x86_64) DMG.
  --background <path>              Optional background image to embed in the DMG.
  --no-ffmpeg                      Build without the ffmpeg backend (uses other defaults).
  --keep-stage                     Keep staging directories instead of deleting them.
  -h, --help                       Show this message.

By default the host architecture is used.
EOF
}

log() {
    echo "==> $*" >&2
}

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing required tool: $1" >&2
        exit 1
    fi
}

prepare_search_paths() {
    # Ensure Xcode linker search paths exist to avoid warnings on single-arch builds.
    mkdir -p \
        "$ROOT/target/release" \
        "$ROOT/target/aarch64-apple-darwin/release" \
        "$ROOT/target/x86_64-apple-darwin/release" \
        "$ROOT/target/universal/release"
}

target_triple() {
    case "$1" in
        arm64) echo "aarch64-apple-darwin" ;;
        x86_64) echo "x86_64-apple-darwin" ;;
        *) echo "Unsupported architecture: $1" >&2; exit 1 ;;
    esac
}

build_rust_arch() {
    local arch="$1"
    local target
    target=$(target_triple "$arch")
    log "Building Rust library ($arch)"
    local cargo_args=(--release --target "$target")
    if [[ "$FFMPEG_BACKEND" -eq 0 ]]; then
        cargo_args+=(--no-default-features -p subtitle-fast -p subtitle-fast-decoder --features "$NO_FFMPEG_FEATURES")
    else
        cargo_args+=(-p subtitle-fast)
    fi
    cargo build "${cargo_args[@]}" >&2
    echo "$ROOT/target/$target/release/libsubtitle_fast.dylib"
}

build_rust_variant() {
    local variant="$1"
    if [[ "$variant" == "universal" ]]; then
        local arm_lib x86_lib fat_lib
        arm_lib=$(build_rust_arch "arm64")
        x86_lib=$(build_rust_arch "x86_64")
        fat_lib="$ROOT/target/universal/release/libsubtitle_fast.dylib"
        mkdir -p "$(dirname "$fat_lib")"
        log "Creating universal Rust dylib"
        lipo -create -output "$fat_lib" "$arm_lib" "$x86_lib"
        echo "$fat_lib"
    else
        build_rust_arch "$variant"
    fi
}

build_macos_app() {
    local variant="$1"
    shift
    local archs=("$@")
    local derived="$DERIVED_DATA/$variant"
    local arch_flags=()
    for arch in "${archs[@]}"; do
        arch_flags+=("-arch" "$arch")
    done

    log "Building macOS app (Release, ${archs[*]})"
    prepare_search_paths
    xcodebuild \
        -project "$PROJECT" \
        -scheme "$SCHEME" \
        -configuration Release \
        -derivedDataPath "$derived" \
        "${arch_flags[@]}" \
        ONLY_ACTIVE_ARCH=NO \
        -quiet >&2

    local app_bundle="$derived/Build/Products/Release/subtitle-fast.app"
    if [[ ! -d "$app_bundle" ]]; then
        echo "App bundle not found at $app_bundle" >&2
        exit 1
    fi
    echo "$app_bundle"
}

bundle_rust_dylib() {
    local app_bundle="$1"
    local rust_lib="$2"
    local frameworks_dir="$app_bundle/Contents/Frameworks"
    local app_binary="$app_bundle/Contents/MacOS/subtitle-fast"
    local bundled_lib="$frameworks_dir/libsubtitle_fast.dylib"

    log "Bundling Rust dylib into app"
    mkdir -p "$frameworks_dir"
    cp "$rust_lib" "$bundled_lib"

    if [[ -f "$app_binary" ]]; then
        install_name_tool -add_rpath "@executable_path/../Frameworks" "$app_binary" 2>/dev/null || true
        install_name_tool -change "$rust_lib" "@rpath/libsubtitle_fast.dylib" "$app_binary" 2>/dev/null || true
    fi
    install_name_tool -id "@rpath/libsubtitle_fast.dylib" "$bundled_lib" 2>/dev/null || true
}

cleanup_current_mount() {
    if [[ -n "${CURRENT_MOUNT:-}" && -d "${CURRENT_MOUNT:-}" ]]; then
        hdiutil detach "${CURRENT_MOUNT}" -quiet >/dev/null 2>&1 || true
    fi
    if [[ -n "${CURRENT_DEV:-}" ]]; then
        hdiutil detach "${CURRENT_DEV}" -quiet >/dev/null 2>&1 || true
    fi
}

create_dmg() {
    local variant="$1"
    local app_bundle="$2"
    local stage_dir="$DIST_DIR/stage-$variant"
    local dmg_path="$DIST_DIR/subtitle-fast-$variant.dmg"
    local tmp_dmg="$DIST_DIR/subtitle-fast-$variant-tmp.dmg"
    local mount_point="$DIST_DIR/mount-$variant"
    local volname="$VOLNAME_BASE"

    if [[ "$variant" != "$(uname -m)" || "${#VARIANTS[@]}" -gt 1 || "$variant" == "universal" ]]; then
        volname="$VOLNAME_BASE-$variant"
    fi

    log "Staging app for DMG ($variant)"
    rm -rf "$stage_dir"
    mkdir -p "$stage_dir"
    cp -R "$app_bundle" "$stage_dir/"
    ln -s /Applications "$stage_dir/Applications"

    local bg_mount_path=""
    if [[ -n "$BACKGROUND_IMAGE" && -f "$BACKGROUND_IMAGE" ]]; then
        mkdir -p "$stage_dir/.background"
        cp "$BACKGROUND_IMAGE" "$stage_dir/.background/background.png"
        bg_mount_path="$mount_point/.background/background.png"
    elif [[ -n "$BACKGROUND_IMAGE" ]]; then
        log "Background image not found at $BACKGROUND_IMAGE (skipping)"
    fi

    mkdir -p "$DIST_DIR"
    rm -f "$dmg_path" "$tmp_dmg"

    log "Creating writable DMG at $tmp_dmg"
    hdiutil create -volname "$volname" -srcfolder "$stage_dir" -ov -format UDRW "$tmp_dmg"

    mkdir -p "$mount_point"
    log "Customizing DMG layout"
    local attach_info
    attach_info=$(hdiutil attach "$tmp_dmg" -readwrite -noverify -noautoopen -mountpoint "$mount_point" 2>/dev/null || true)
    local dev_name
    dev_name=$(echo "$attach_info" | awk '/Apple_HFS/ {print $1; exit}')

    CURRENT_MOUNT="$mount_point"
    CURRENT_DEV="$dev_name"
    trap cleanup_current_mount EXIT

    if [[ -n "$dev_name" && -d "$mount_point" ]]; then
        /usr/bin/osascript <<APPLESCRIPT
tell application "Finder"
    tell disk "$volname"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set status bar visible of container window to false
        set the bounds of container window to {100, 100, 680, 480}
        set viewOptions to the icon view options of container window
        set icon size of viewOptions to 96
        set arrangement of viewOptions to not arranged
        if "$bg_mount_path" is not "" then
            try
                set background picture of viewOptions to POSIX file "$bg_mount_path"
            end try
        end if
        set appsAlias to missing value
        try
            set appsAlias to item "Applications" of container window
        on error
            set appsAlias to make alias file to POSIX file "/Applications" at container window
            set name of appsAlias to "Applications"
        end try
        set position of item "subtitle-fast.app" of container window to {140, 240}
        set position of item "Applications" of container window to {420, 240}
        update without registering applications
        delay 1
        close
    end tell
end tell
APPLESCRIPT

        sync
    else
        echo "Could not attach DMG for customization" >&2
    fi

    log "Converting to compressed DMG"
    cleanup_current_mount
    trap - EXIT

    hdiutil convert "$tmp_dmg" -format UDZO -imagekey zlib-level=9 -o "$dmg_path" >/dev/null
    rm -f "$tmp_dmg"

    if [[ "$KEEP_STAGE" -eq 0 ]]; then
        rm -rf "$stage_dir" "$mount_point"
    fi
    log "Done. DMG ready at $dmg_path"
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --arch)
                shift
                [[ $# -gt 0 ]] || usage
                case "$1" in
                    arm64|x86_64|universal) VARIANTS+=("$1") ;;
                    *) echo "Unsupported arch: $1" >&2; usage ;;
                esac
                shift
                ;;
            --all)
                VARIANTS+=("arm64" "x86_64")
                shift
                ;;
            --universal)
                VARIANTS+=("universal")
                shift
                ;;
            --background)
                shift
                [[ $# -gt 0 ]] || usage
                BACKGROUND_IMAGE="$1"
                shift
                ;;
            --keep-stage)
                KEEP_STAGE=1
                shift
                ;;
            --no-ffmpeg)
                FFMPEG_BACKEND=0
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                echo "Unknown option: $1" >&2
                usage
                exit 1
                ;;
        esac
    done
}

dedupe_variants() {
    local seen=()
    local unique=()
    for variant in "${VARIANTS[@]}"; do
        local duplicate=0
        for item in "${seen[@]}"; do
            if [[ "$item" == "$variant" ]]; then
                duplicate=1
                break
            fi
        done
        if [[ "$duplicate" -eq 0 ]]; then
            seen+=("$variant")
            unique+=("$variant")
        fi
    done
    VARIANTS=("${unique[@]}")
}

main() {
    parse_args "$@"

    if [[ ${#VARIANTS[@]} -eq 0 ]]; then
        local host_arch
        host_arch=$(uname -m)
        case "$host_arch" in
            arm64|x86_64) VARIANTS=("$host_arch") ;;
            *) echo "Unsupported host architecture: $host_arch" >&2; exit 1 ;;
        esac
    fi

    dedupe_variants

    require_cmd cargo
    require_cmd xcodebuild
    require_cmd hdiutil
    require_cmd install_name_tool
    require_cmd lipo
    require_cmd osascript

    for variant in "${VARIANTS[@]}"; do
        local archs=()
        if [[ "$variant" == "universal" ]]; then
            archs=("arm64" "x86_64")
        else
            archs=("$variant")
        fi

        local rust_lib app_bundle
        rust_lib=$(build_rust_variant "$variant")
        app_bundle=$(build_macos_app "$variant" "${archs[@]}")
        bundle_rust_dylib "$app_bundle" "$rust_lib"
        create_dmg "$variant" "$app_bundle"
    done
}

main "$@"
