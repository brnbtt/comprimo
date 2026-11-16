#!/usr/bin/env bash
# Builds the minimal, static ffmpeg.exe that comprimo downloads on install.
#
# It only contains what comprimo uses: decoders for common video/audio formats,
# H.264 (libx264) + AAC encoding, MP4 output and the scale/fps filters.
# That makes it a few MB instead of ~100 MB for a full build.
#
# Runs on Linux (Ubuntu) and cross-compiles for Windows x64 with mingw-w64:
#   sudo apt-get install gcc mingw-w64 nasm meson ninja-build cmake make pkg-config git curl xz-utils zip
#   ./build.sh            # -> out/comprimo-ffmpeg-win64.zip
set -euo pipefail

FFMPEG_VERSION=9.0.2
DAV1D_VERSION=1.5.4
X264_COMMIT=b35605ace3ddf7c1a5d67a2eb553f034aef41d55 # x264 "stable" branch
# GPU encoders. Older header versions on purpose: newer ones require newer GPU drivers.
NVCODEC_VERSION=n12.1.14.0 # NVIDIA NVENC (driver 531+)
AMF_VERSION=v1.5.3         # AMD AMF (ffmpeg 9 needs >= 1.5.2)
LIBVPL_VERSION=v2.17.0     # Intel Quick Sync

HOST=x86_64-w64-mingw32
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-$HERE/work}"
OUT="${OUT:-$HERE/out}"
PREFIX="$WORK/prefix"
JOBS="$(nproc)"
mkdir -p "$WORK" "$OUT" "$PREFIX"
cd "$WORK"

export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_PATH="$PKG_CONFIG_LIBDIR"

# --- x264 ---------------------------------------------------------------------
if [ ! -f "$PREFIX/lib/libx264.a" ]; then
    rm -rf x264
    git init -q x264 && git -C x264 fetch -q --depth 1 https://code.videolan.org/videolan/x264.git "$X264_COMMIT"
    git -C x264 checkout -q FETCH_HEAD
    (cd x264 && ./configure --host="$HOST" --cross-prefix="$HOST-" --prefix="$PREFIX" \
        --enable-static --disable-cli --disable-opencl --enable-strip \
        && make -j"$JOBS" && make install)
fi

# --- dav1d (AV1 decoder) ------------------------------------------------------
if [ ! -f "$PREFIX/lib/libdav1d.a" ]; then
    rm -rf dav1d
    git clone -q --depth 1 --branch "$DAV1D_VERSION" https://code.videolan.org/videolan/dav1d.git
    cat > cross.txt <<EOF
[binaries]
c = '$HOST-gcc'
cpp = '$HOST-g++'
ar = '$HOST-ar'
strip = '$HOST-strip'
windres = '$HOST-windres'
[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF
    meson setup dav1d/build dav1d --cross-file cross.txt --prefix="$PREFIX" --libdir=lib \
        --default-library=static --buildtype=release -Denable_tools=false -Denable_tests=false
    ninja -C dav1d/build install
fi

# --- GPU encoder SDKs (headers + Intel's small dispatcher; drivers are loaded at runtime) ---
if [ ! -f "$PREFIX/include/ffnvcodec/nvEncodeAPI.h" ]; then
    rm -rf nv-codec-headers
    git clone -q --depth 1 --branch "$NVCODEC_VERSION" https://github.com/FFmpeg/nv-codec-headers.git
    make -C nv-codec-headers install PREFIX="$PREFIX"
fi
if [ ! -f "$PREFIX/include/AMF/core/Version.h" ]; then
    rm -rf AMF
    git clone -q --depth 1 --filter=blob:none --sparse --branch "$AMF_VERSION" https://github.com/GPUOpen-LibrariesAndSDKs/AMF.git
    git -C AMF sparse-checkout set amf/public/include
    mkdir -p "$PREFIX/include/AMF"
    cp -r AMF/amf/public/include/* "$PREFIX/include/AMF/"
fi
if [ ! -f "$PREFIX/lib/libvpl.a" ]; then
    rm -rf libvpl
    git clone -q --depth 1 --branch "$LIBVPL_VERSION" https://github.com/intel/libvpl.git
    cmake -S libvpl -B libvpl/build -DCMAKE_SYSTEM_NAME=Windows -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_C_COMPILER="$HOST-gcc" -DCMAKE_CXX_COMPILER="$HOST-g++" -DCMAKE_RC_COMPILER="$HOST-windres" \
        -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_SHARED_LIBS=OFF \
        -DBUILD_TOOLS=OFF -DBUILD_EXAMPLES=OFF -DINSTALL_EXAMPLES=OFF -DBUILD_TESTS=OFF
    cmake --build libvpl/build -j"$JOBS"
    cmake --install libvpl/build
fi

# --- ffmpeg -------------------------------------------------------------------
if [ ! -d "ffmpeg-$FFMPEG_VERSION" ]; then
    curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" | tar xJ
fi

join() { local IFS=,; echo "$*"; }

DECODERS=(
    # video
    h264 hevc vp8 vp9 libdav1d mpeg4 msmpeg4v1 msmpeg4v2 msmpeg4v3 mpeg1video mpeg2video
    wmv1 wmv2 wmv3 vc1 flv h263 h263p prores mjpeg dnxhd theora ffv1 utvideo huffyuv ffvhuff
    rawvideo qtrle gif png cfhd
    # audio
    aac aac_latm mp3 mp3float mp2 mp2float opus vorbis ac3 eac3 flac alac wmav1 wmav2 wmapro
    dca truehd amrnb amrwb adpcm_ima_wav pcm_s16le pcm_s16be pcm_s24le pcm_s24be pcm_s32le
    pcm_f32le pcm_f32be pcm_u8 pcm_alaw pcm_mulaw
)
DEMUXERS=(mov matroska avi asf flv mpegts mpegps mpegvideo ogg wav mp3 aac h264 hevc m4v ivf gif)
PARSERS=(h264 hevc vp8 vp9 av1 aac aac_latm mpegaudio ac3 mpeg4video mpegvideo opus vorbis flac dca vc1 h263 mjpeg png gif)
BSFS=(aac_adtstoasc extract_extradata vp9_superframe_split av1_frame_split h264_mp4toannexb hevc_mp4toannexb null)
FILTERS=(scale fps format null anull aformat aresample transpose hflip vflip copy setsar trim atrim
         buffer buffersink abuffer abuffersink)

cd "ffmpeg-$FFMPEG_VERSION"
./configure \
    --prefix="$PREFIX" --arch=x86_64 --target-os=mingw32 --cross-prefix="$HOST-" \
    --pkg-config=pkg-config --pkg-config-flags=--static \
    --enable-static --disable-shared --extra-ldflags=-static \
    --disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug \
    --disable-programs --enable-ffmpeg --disable-avdevice \
    --enable-w32threads --enable-gpl --enable-libx264 --enable-libdav1d \
    --enable-ffnvcodec --enable-nvenc --enable-amf --enable-libvpl --enable-d3d11va --enable-dxva2 \
    --extra-libs=-lstdc++ \
    --enable-protocol=file,pipe \
    --enable-encoder=libx264,aac,wrapped_avframe,h264_nvenc,h264_amf,h264_qsv \
    --enable-muxer=mp4,null \
    --enable-decoder="$(join "${DECODERS[@]}")" \
    --enable-demuxer="$(join "${DEMUXERS[@]}")" \
    --enable-parser="$(join "${PARSERS[@]}")" \
    --enable-bsf="$(join "${BSFS[@]}")" \
    --enable-filter="$(join "${FILTERS[@]}")"
make -j"$JOBS"
"$HOST-strip" ffmpeg.exe

# --- package --------------------------------------------------------------------
STAGE="$WORK/stage"
rm -rf "$STAGE" && mkdir -p "$STAGE"
cp ffmpeg.exe "$STAGE/"
cp COPYING.GPLv3 "$STAGE/LICENSE.txt"
cat > "$STAGE/README.txt" <<EOF
Minimal ffmpeg build used by comprimo (https://github.com/brnbtt/comprimo).

ffmpeg $FFMPEG_VERSION  https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz
x264   $X264_COMMIT  https://code.videolan.org/videolan/x264
dav1d  $DAV1D_VERSION  https://code.videolan.org/videolan/dav1d
nv-codec-headers $NVCODEC_VERSION, AMF headers $AMF_VERSION, libvpl $LIBVPL_VERSION (GPU encoders)

Built with ffmpeg-slim/build.sh from the comprimo repository. Licensed under the GPL.
EOF
rm -f "$OUT/comprimo-ffmpeg-win64.zip"
(cd "$STAGE" && zip -9 -q "$OUT/comprimo-ffmpeg-win64.zip" ffmpeg.exe LICENSE.txt README.txt)
ls -l "$STAGE/ffmpeg.exe" "$OUT/comprimo-ffmpeg-win64.zip"
