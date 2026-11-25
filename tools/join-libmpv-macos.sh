#!/usr/bin/env bash
# Joins out/libmpv-arm64 and out/libmpv-x86_64 (tools/build-libmpv-macos.sh) into the universal out/libmpv.2.dylib
# and checks it: both architectures, the install name, nothing linked outside the system, the render API exported.
# Run from the repo root.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p out
lipo -create -output out/libmpv.2.dylib out/libmpv-arm64/libmpv.2.dylib out/libmpv-x86_64/libmpv.2.dylib
cp out/libmpv-*/BUILDINFO-*.txt out/
D=out/libmpv.2.dylib
O=out/BUILDINFO.txt
{
  echo "== architectures"; lipo -archs "$D"
  echo; echo "== install name"; otool -D "$D" | tail -1
  echo; echo "== linked libraries"; otool -L "$D"
  echo; echo "== exports"; nm -gU "$D" | grep -o '_mpv_[a-z_0-9]*' | sort -u
  echo; echo "size: $(stat -f %z "$D")"
} | tee "$O"
[ "$(lipo -archs "$D")" = "x86_64 arm64" ]
[ "$(otool -D "$D" | tail -1)" = "@rpath/libmpv.2.dylib" ]
# (self-contained: only the system's frameworks and libraries, nothing from Homebrew)
if otool -L "$D" | grep -E '^\s' | grep -v -E '^\s+(/System/Library/|/usr/lib/|@rpath/libmpv\.2\.dylib)'; then
  echo "libmpv links outside the system"; exit 1
fi
# (listed once: grep -q stops reading early, and nm's broken pipe would fail the step under pipefail)
exports=$(nm -gU "$D")
for f in mpv_render_context_create mpv_render_context_render mpv_render_context_update \
         mpv_render_context_set_update_callback mpv_render_context_free mpv_render_context_report_swap; do
  grep -q "_$f$" <<< "$exports" || { echo "missing export $f"; exit 1; }
done
