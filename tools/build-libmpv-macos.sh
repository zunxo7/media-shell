#!/usr/bin/env bash
# One architecture of libmpv.2.dylib for the Mac app (docs/plans/mac-video.md), run by codemagic.yaml's
# macos-libmpv and macos-app workflows: the Windows recipe (mpv v0.42.0, FFmpeg n9.0.2, LGPL, the same decoders,
# demuxers and protocols) built on macOS. Usage: tools/build-libmpv-macos.sh arm64|x86_64   (from the repo root)
#
# Mac differences from Windows:
#   - video: OpenGL for the render API (gl-cocoa, plain-gl) with VideoToolbox decoding (videotoolbox-gl); no d3d11,
#     no Vulkan, no shaderc or SPIRV-Cross, libplacebo without a GPU backend (vo_gpu draws for the OpenGL render API).
#   - TLS: libcurl on OpenSSL 3 with USE_APPLE_SECTRUST, so certificates are checked by the system's trust store and
#     no CA bundle ships (curl dropped Secure Transport in 8.15); FFmpeg's own https on Secure Transport.
#   - sound: CoreAudio. Text for subtitles: libass with CoreText. zlib: the SDK's.
#   - Both halves are built on the Apple Silicon runner, x86_64 by clang's -arch: Intel runners keep Homebrew in
#     /usr/local, which clang and mpv's link flags search, so stray libraries could be picked up there.
#   - Swift is on: mpv 0.42's Cocoa code (needed by gl-cocoa, which VideoToolbox's zero-copy interop needs) includes
#     its Swift bridge even in a library build. Only the base Swift sources are built: no cocoa-cb, media keys or
#     Touch Bar. The Swift runtime is the system's (/usr/lib/swift, in macOS since 10.14.4).
#
# Built dependencies live in ~/zcache/<arch> (a fixed path: the pkg-config files bake it in), keyed by each one's
# recipe, so a rerun only rebuilds what changed. The result is out/libmpv-<arch>/libmpv.2.dylib.
set -euo pipefail
ARCH=${1:?arch: arm64 or x86_64}
cd "$(dirname "$0")/.."
REPO=$PWD
export ARCH
# (a failed mpv configure says why in its log)
trap 'tail -n 300 "$CM_BUILD_DIR/work/$ARCH/src/mpv/build/meson-logs/meson-log.txt" 2>/dev/null || true' ERR
# bump to throw away every cached dependency
export CACHE_EPOCH=1
export MPV_VERSION=v0.42.0
export FFMPEG_VERSION=n9.0.2
export MACOSX_DEPLOYMENT_TARGET=11.0
# (Homebrew's packages must not be found as dependencies: everything comes from the prefix or the SDK)
export CMAKE_IGNORE_PREFIX_PATH='/opt/homebrew;/usr/local'
export CM_BUILD_DIR=${CM_BUILD_DIR:-$PWD}
# each architecture works in its own directory: sources and configs differ
mkdir -p "$CM_BUILD_DIR/work/$ARCH"
cd "$CM_BUILD_DIR/work/$ARCH"

brew install meson ninja nasm ccache
brew list pkgconf >/dev/null 2>&1 || brew install pkgconf

# ---- build environment
(
set -euo pipefail
CACHE=$HOME/zcache/$CACHE_EPOCH/$ARCH
W=$CM_BUILD_DIR/work/$ARCH
P=$CACHE/prefix
mkdir -p "$P/lib/pkgconfig" "$P/include" "$P/.done" "$P/versions" "$W/src"
SDK=$(xcrun --sdk macosx --show-sdk-path)
FLAGS="-arch $ARCH -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET -isysroot $SDK"
OPT="$FLAGS -ffunction-sections -fdata-sections"
machine() { # name, optimisation flag, extra link flags
  q() { printf "'%s', " $1; }
  cat > "$W/$1" <<EOF
[constants]
opt = [$(q "$2 -pipe $OPT")]
link = [$(q "$FLAGS $3")]
[built-in options]
buildtype = 'plain'
wrap_mode = 'nodownload'
default_library = 'static'
prefer_static = true
libdir = 'lib'
c_args = opt
cpp_args = opt
objc_args = opt
c_link_args = link
cpp_link_args = link
objc_link_args = link
[properties]
pkg_config_libdir = '$P/lib/pkgconfig'
[binaries]
c = ['ccache', 'clang']
cpp = ['ccache', 'clang++']
objc = ['ccache', 'clang']
ar = 'ar'
strip = 'strip'
pkg-config = 'pkg-config'
nasm = 'nasm'
EOF
  # (x86_64 is a cross build from the Apple Silicon runner; arm64 is native)
  if [ "$ARCH" = x86_64 ]; then
    printf '%s\n' "[host_machine]" "system = 'darwin'" "cpu_family = 'x86_64'" "cpu = 'x86_64'" \
      "endian = 'little'" >> "$W/$1"
  fi
}
machine fast.ini -O2 ""
machine small.ini -Os ""
# mpv itself: a dylib, every dependency linked in statically, unused sections dropped
machine mpv.ini -O2 "-Wl,-dead_strip"
if [ "$ARCH" = arm64 ]; then MESON_MACHINE=--native-file; else MESON_MACHINE=--cross-file; fi
cat "$W/mpv.ini"
{
  echo "W=$W"; echo "P=$P"; echo "CACHE=$CACHE"; echo "SDK=$SDK"; echo "FLAGS=\"$FLAGS\""; echo "OPT=\"$OPT\""
  echo "MESON_MACHINE=$MESON_MACHINE"
} > "$W/env.sh"
)

# ---- dependencies
(
set -euo pipefail
. ./env.sh
export CCACHE_DIR=$CACHE/ccache CCACHE_MAXSIZE=700M CCACHE_BASEDIR=$W
export PKG_CONFIG_LIBDIR=$P/lib/pkgconfig PKG_CONFIG_PATH=
NCPU=$(sysctl -n hw.ncpu)
cmake_args=(
  -GNinja -Wno-dev
  -DCMAKE_OSX_ARCHITECTURES=$ARCH -DCMAKE_OSX_DEPLOYMENT_TARGET=$MACOSX_DEPLOYMENT_TARGET
  -DCMAKE_OSX_SYSROOT="$SDK"
  -DCMAKE_C_COMPILER_LAUNCHER=ccache -DCMAKE_CXX_COMPILER_LAUNCHER=ccache
  -DCMAKE_PREFIX_PATH="$P" -DCMAKE_INSTALL_PREFIX="$P" -DCMAKE_INSTALL_LIBDIR=lib
  -DCMAKE_C_FLAGS="-ffunction-sections -fdata-sections"
  -DCMAKE_C_FLAGS_RELEASE="-O2 -DNDEBUG" -DCMAKE_C_FLAGS_MINSIZEREL="-Os -DNDEBUG"
  -DBUILD_SHARED_LIBS=OFF
)
gitclone() { # url, tag, dir
  [ -d "$W/src/$3" ] || git clone --depth=1 --recursive --shallow-submodules --branch "$2" "$1" "$W/src/$3"
  git -C "$W/src/$3" describe --tags --always > "$P/versions/$3.txt" 2>/dev/null || echo "$2" > "$P/versions/$3.txt"
}
gettar() { # url, dir
  if [ ! -d "$W/src/$2" ]; then
    curl -fsSL "$1" -o "$W/src/dl.tar" && tar -xf "$W/src/dl.tar" -C "$W/src" && rm "$W/src/dl.tar" || return 1
  fi
  [ -d "$W/src/$2" ] || return 1
  echo "$2" > "$P/versions/$2.txt"
}
fresh() { rm -rf "$1/zbuild"; mkdir -p "$1/zbuild"; cd "$1/zbuild"; }

_dav1d() {
  gitclone https://code.videolan.org/videolan/dav1d.git 1.5.4 dav1d
  fresh "$W/src/dav1d"
  meson setup .. $MESON_MACHINE "$W/fast.ini" --prefix="$P" \
    -Denable_tools=false -Denable_tests=false -Denable_examples=false -Denable_docs=false
  ninja install
}
_nghttp2() {
  gettar https://github.com/nghttp2/nghttp2/releases/download/v1.71.0/nghttp2-1.71.0.tar.xz nghttp2-1.71.0
  fresh "$W/src/nghttp2-1.71.0"
  cmake .. "${cmake_args[@]}" -DCMAKE_BUILD_TYPE=MinSizeRel -DENABLE_LIB_ONLY=ON \
    -DBUILD_STATIC_LIBS=ON -DENABLE_DOC=OFF -DBUILD_TESTING=OFF
  ninja install
}
_openssl() {
  gettar https://github.com/openssl/openssl/releases/download/openssl-3.5.9/openssl-3.5.9.tar.gz openssl-3.5.9
  fresh "$W/src/openssl-3.5.9"
  [ "$ARCH" = arm64 ] && local target=darwin64-arm64-cc || local target=darwin64-x86_64-cc
  # (only libcrypto and libssl for curl: no apps, engines, modules or legacy ciphers)
  CC="ccache clang" ../Configure "$target" --prefix="$P" --libdir=lib --openssldir=/private/etc/ssl \
    no-shared no-module no-engine no-legacy no-apps no-docs no-tests no-ui-console no-comp \
    -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET -isysroot "$SDK" -ffunction-sections -fdata-sections
  make -j"$NCPU" build_libs
  make install_dev
}
_curl() {
  gettar https://curl.se/download/curl-8.23.0.tar.xz curl-8.23.0
  fresh "$W/src/curl-8.23.0"
  cmake .. "${cmake_args[@]}" -DCMAKE_BUILD_TYPE=MinSizeRel \
    -DBUILD_STATIC_LIBS=ON -DBUILD_CURL_EXE=OFF -DBUILD_TESTING=OFF -DBUILD_LIBCURL_DOCS=OFF \
    -DBUILD_MISC_DOCS=OFF -DENABLE_CURL_MANUAL=OFF -DBUILD_EXAMPLES=OFF \
    -DHTTP_ONLY=ON -DCURL_DISABLE_LDAP=ON \
    -DCURL_USE_OPENSSL=ON -DOPENSSL_ROOT_DIR="$P" -DOPENSSL_USE_STATIC_LIBS=ON -DUSE_APPLE_SECTRUST=ON \
    -DCURL_CA_BUNDLE=none -DCURL_CA_PATH=none \
    -DUSE_NGHTTP2=ON -DCURL_ZLIB=ON \
    -DCURL_BROTLI=OFF -DCURL_ZSTD=OFF -DCURL_USE_LIBSSH2=OFF -DCURL_USE_LIBSSH=OFF \
    -DCURL_USE_LIBPSL=OFF -DUSE_LIBIDN2=OFF -DUSE_APPLE_IDN=OFF -DUSE_NGTCP2=OFF -DCURL_USE_GSSAPI=OFF
  ninja install
  grep -q AppleSecTrust "$P/lib/pkgconfig/libcurl.pc" || grep -q -i sectrust lib/curl_config.h
}
_ffmpeg() {
  gitclone https://github.com/FFmpeg/FFmpeg.git "$FFMPEG_VERSION" ffmpeg
  fresh "$W/src/ffmpeg"
  local cross=() hw=() c
  [ "$ARCH" = x86_64 ] && cross=(--enable-cross-compile --arch=x86_64 --target-os=darwin)
  for c in h264 hevc av1 vp9; do hw+=(${c}_videotoolbox); done
  # (--disable-autodetect: nothing found by chance, from Homebrew or elsewhere; what's used is named)
  local args=(
    --prefix="$P" ${cross[@]+"${cross[@]}"} --cc="ccache clang" --cxx="ccache clang++" --x86asmexe=nasm
    --pkg-config=pkg-config --pkg-config-flags=--static
    --extra-cflags="$OPT -I$P/include" --extra-ldflags="$FLAGS -L$P/lib"
    --enable-static --disable-shared --disable-programs --disable-doc --disable-debug
    --disable-avdevice --disable-everything --disable-autodetect
    --enable-pthreads --enable-network --enable-zlib --enable-securetransport --enable-libdav1d
    --enable-videotoolbox --enable-audiotoolbox
    --enable-decoder=h264,hevc,av1,libdav1d,vp9,vp8,aac,aac_latm,opus,ac3,eac3,mp3,mp3float,mp2,mp2float,flac,dca,truehd,mlp,vorbis,alac,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_f32le,pcm_bluray,pcm_dvd,mjpeg,png,ass,ssa,subrip,webvtt,mov_text
    --enable-parser=h264,hevc,av1,vp9,vp8,aac,aac_latm,opus,ac3,mpegaudio,flac,dca,mlp,vorbis,mjpeg,png
    --enable-demuxer=hls,mov,matroska,mpegts,aac,mp3,ac3,eac3,flac,ogg,wav,dts,truehd,webvtt
    --enable-protocol=file,http,https,tcp,tls,crypto,hls,data,httpproxy
    --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,vp9_superframe_split,av1_frame_split,extract_extradata,null
    --enable-filter=aresample,aformat,format,scale,anull,null,yadif,bwdif
    --enable-hwaccel=$(IFS=,; echo "${hw[*]}")
  )
  ../configure "${args[@]}" | tee "$P/versions/ffmpeg-configure-out.txt"
  printf '%s\n' "${args[@]}" > "$P/versions/ffmpeg-configure.txt"
  for c in videotoolbox securetransport; do
    grep -q "^#define CONFIG_$(echo $c | tr a-z A-Z) 1" config.h || { echo "FFmpeg: $c is off"; exit 1; }
  done
  make -j"$NCPU"
  make install
}
_libplacebo() {
  gitclone https://code.videolan.org/videolan/libplacebo.git v7.387.0 libplacebo
  fresh "$W/src/libplacebo"
  meson setup .. $MESON_MACHINE "$W/fast.ini" --prefix="$P" \
    -Dvulkan=disabled -Dopengl=disabled -Dd3d11=disabled -Dshaderc=disabled -Dglslang=disabled \
    -Dlcms=disabled -Ddovi=enabled -Dlibdovi=disabled -Dunwind=disabled -Dxxhash=disabled \
    -Ddemos=false -Dtests=false -Dbench=false -Dfuzz=false
  ninja install
  grep -q 'lc++' "$P/lib/pkgconfig/libplacebo.pc" || sed -i '' 's/^Libs: \(.*\)$/Libs: \1 -lc++/' "$P/lib/pkgconfig/libplacebo.pc"
}
_freetype() {
  gettar https://download.savannah.gnu.org/releases/freetype/freetype-2.14.3.tar.xz freetype-2.14.3 || \
    gettar https://sourceforge.net/projects/freetype/files/freetype2/2.14.3/freetype-2.14.3.tar.xz/download freetype-2.14.3
  fresh "$W/src/freetype-2.14.3"
  meson setup .. $MESON_MACHINE "$W/small.ini" --prefix="$P" \
    -Dbrotli=disabled -Dbzip2=disabled -Dharfbuzz=disabled -Dpng=disabled -Dtests=disabled
  ninja install
}
_fribidi() {
  gettar https://github.com/fribidi/fribidi/releases/download/v1.0.16/fribidi-1.0.16.tar.xz fribidi-1.0.16
  fresh "$W/src/fribidi-1.0.16"
  meson setup .. $MESON_MACHINE "$W/small.ini" --prefix="$P" -Ddocs=false -Dtests=false -Dbin=false
  ninja install
}
_harfbuzz() {
  gettar https://github.com/harfbuzz/harfbuzz/releases/download/14.2.0/harfbuzz-14.2.0.tar.xz harfbuzz-14.2.0
  fresh "$W/src/harfbuzz-14.2.0"
  meson setup .. $MESON_MACHINE "$W/small.ini" --prefix="$P" \
    -Dtests=disabled -Ddocs=disabled -Dutilities=disabled -Dglib=disabled -Dgobject=disabled \
    -Dcairo=disabled -Dicu=disabled -Dfreetype=enabled -Dcoretext=disabled -Dintrospection=disabled
  ninja install
}
_libass() {
  gitclone https://github.com/libass/libass.git 0.17.5 libass
  fresh "$W/src/libass"
  meson setup .. $MESON_MACHINE "$W/small.ini" --prefix="$P" \
    -Dfontconfig=disabled -Ddirectwrite=disabled -Dcoretext=enabled -Drequire-system-font-provider=true \
    -Dasm=enabled -Dlibunibreak=disabled -Dtest=disabled -Dcompare=disabled -Dprofile=disabled \
    -Dfuzz=disabled -Dcheckasm=disabled
  ninja install
}

base=$(cat "$W"/*.ini; echo "${cmake_args[*]}")
build() {
  local h
  h=$( { declare -f "_$1"; echo "$base"; } | md5 | cut -c1-12)
  if [ -e "$P/.done/$1-$h" ]; then echo "$1: cached"; return; fi
  echo "::group::$1"
  ( "_$1" )
  rm -f "$P/.done/$1"-*; touch "$P/.done/$1-$h"
  echo "::endgroup::"
}
for x in dav1d nghttp2 openssl curl ffmpeg libplacebo freetype fribidi harfbuzz libass; do
  build $x
done
ccache -s || true
)

# ---- libmpv
(
set -euo pipefail
. ./env.sh
export CCACHE_DIR=$CACHE/ccache CCACHE_MAXSIZE=700M CCACHE_BASEDIR=$W
export PKG_CONFIG_LIBDIR=$P/lib/pkgconfig PKG_CONFIG_PATH=
# zlib is the SDK's, which ships no .pc, yet libcurl's .pc requires one
ZV=$(sed -n 's/^#define ZLIB_VERSION "\(.*\)"/\1/p' "$SDK/usr/include/zlib.h")
printf '%s\n' "Name: zlib" "Description: zlib from the macOS SDK" "Version: $ZV" "Libs: -lz" "Cflags:" \
  > "$P/lib/pkgconfig/zlib.pc"
git clone --depth=1 --branch "$MPV_VERSION" https://github.com/mpv-player/mpv.git "$W/src/mpv"
cd "$W/src/mpv"
# mpv's fix for a Swift build without Vulkan, after 0.42.0 (f32e7ef1e8, "mac/view: fix a libmpv build
# failure under macOS with Vulkan disabled")
sed -i '' 's/layer as? MetalLayer/layer as? CAMetalLayer/' video/out/mac/view.swift
grep -q 'as? CAMetalLayer' video/out/mac/view.swift
off=(
  cplayer=false tests=false fuzzers=false build-date=false gpl=false
  lua=disabled javascript=disabled cplugins=disabled vapoursynth=disabled libarchive=disabled
  libbluray=disabled dvdnav=disabled cdda=disabled dvbin=disabled dvda=disabled jack=disabled
  openal=disabled caca=disabled sixel=disabled vulkan=disabled d3d11=disabled shaderc=disabled
  spirv-cross=disabled amf=disabled lcms2=disabled iconv=disabled zimg=disabled rubberband=disabled
  uchardet=disabled jpeg=disabled libavdevice=disabled subrandr=disabled sdl2-audio=disabled
  sdl2-video=disabled sdl2-gamepad=disabled audiounit=disabled avfoundation=disabled
  macos-cocoa-cb=disabled macos-media-player=disabled macos-touchbar=disabled
  videotoolbox-pl=disabled x11=disabled wayland=disabled egl=disabled
  manpage-build=disabled html-build=disabled pdf-build=disabled
)
on=(
  libmpv=true cocoa=enabled swift-build=enabled gl=enabled gl-cocoa=enabled plain-gl=enabled videotoolbox-gl=enabled
  coreaudio=enabled libcurl=enabled zlib=enabled
)
meson setup build $MESON_MACHINE "$W/mpv.ini" --prefix="$W/out" -Ddefault_library=shared \
  -Dswift-flags="-target $ARCH-apple-macos$MACOSX_DEPLOYMENT_TARGET" \
  $(printf -- '-D%s ' "${off[@]}" "${on[@]}") | tee "$W/mpv-setup.txt"
meson compile -C build
ls -la build/*.dylib
)

# ---- package
(
set -euo pipefail
. ./env.sh
D=$REPO/out/libmpv-$ARCH
mkdir -p "$D"
cp "$(ls src/mpv/build/libmpv.2*.dylib | head -1)" "$D/libmpv.2.dylib"
install_name_tool -id @rpath/libmpv.2.dylib "$D/libmpv.2.dylib"
# (mpv's Swift link adds the build machine's toolchain as an rpath; the runtime is /usr/lib/swift)
for r in $(otool -l "$D/libmpv.2.dylib" | awk '/LC_RPATH/{getline; getline; print $2}'); do
  case $r in /usr/lib/*) ;; *) install_name_tool -delete_rpath "$r" "$D/libmpv.2.dylib";; esac
done
strip -x "$D/libmpv.2.dylib"
O="$D/BUILDINFO-$ARCH.txt"
{
  echo "libmpv.2.dylib, trimmed, $ARCH, macOS $MACOSX_DEPLOYMENT_TARGET+, static dependencies (one dylib)"
  echo "Run: codemagic build ${CM_BUILD_ID:-local}"
  echo "mpv: $(git -C src/mpv describe --tags --always)"
  echo "Compiler: $(clang --version | head -1)"
  echo "SDK: $(xcrun --sdk macosx --show-sdk-version)"
  echo
  echo "== mpv features"
  grep -E "List of enabled features|Message: List" "$W/mpv-setup.txt" || true
  echo
  echo "== dependency versions"
  for f in "$P"/versions/*.txt; do case $f in *configure*) ;; *) echo "$(basename "$f" .txt): $(cat "$f")";; esac; done
  echo
  echo "== FFmpeg configure"
  cat "$P/versions/ffmpeg-configure.txt"
  echo
  echo "== FFmpeg enabled components"
  sed -n '/^Enabled decoders/,/^License/p' "$P/versions/ffmpeg-configure-out.txt"
} > "$O"
cat "$O"
)
