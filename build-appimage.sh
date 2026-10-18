#!/usr/bin/env bash
# Builds dist/MusicPlayer-x86_64.AppImage: the whole player in one file that runs on
# most Linux systems from 2022 on (glibc 2.35 or newer), nothing to install.
#
#   ./build-appimage.sh
#
# Needs Rust (https://rustup.rs), curl and python3. The first build downloads the
# libraries and takes a few minutes; after you change something it takes seconds.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

BUILD=build                          # downloads and the unpacked app (safe to delete)
APPDIR="$BUILD/AppDir"
OUT=dist/MusicPlayer-x86_64.AppImage
GITHUB=https://github.com/AppImage

mkdir -p "$BUILD" dist

echo "Compiling..."
cargo build --release

get() {                              # get URL FILE: downloads once, then uses the saved copy
    if [ ! -s "$2" ]; then
        echo "Downloading $(basename "$2")..."
        curl -fL --progress-bar -o "$2.part" "$1" && mv "$2.part" "$2"
    fi
}
# The tool that packs AppImages, and the small program at the start of every AppImage
get "$GITHUB/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" "$BUILD/appimagetool"
get "$GITHUB/type2-runtime/releases/download/continuous/runtime-x86_64" "$BUILD/runtime-x86_64"
chmod +x "$BUILD/appimagetool"

# The app folder: the program, its menu entry and icon
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/doc/music-player"
cp target/release/music-player "$APPDIR/usr/bin/"
python3 packaging/weak-glibc.py "$APPDIR/usr/bin/music-player"   # (see the explanation inside)
ln -s usr/bin/music-player "$APPDIR/AppRun"
cp packaging/music-player.desktop "$APPDIR/"
cp assets/icon.png "$APPDIR/music-player.png"
ln -s music-player.png "$APPDIR/.DirIcon"
cp assets/Inter-LICENSE.txt "$APPDIR/usr/share/doc/music-player/"

echo "Packing..."
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$BUILD/appimagetool" --no-appstream \
    --runtime-file "$BUILD/runtime-x86_64" "$APPDIR" "$OUT" > "$BUILD/appimagetool.log" 2>&1 \
    || { cat "$BUILD/appimagetool.log"; exit 1; }
chmod +x "$OUT"
echo "Done: $OUT ($(du -h "$OUT" | cut -f1))"
