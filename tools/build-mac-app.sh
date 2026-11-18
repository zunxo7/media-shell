#!/usr/bin/env bash
# The Mac app (decision 0009), run by codemagic.yaml's macos-app workflow from the repo root: both halves built and
# joined into one universal app, packed as an installer package (.pkg), opened on the runner and photographed.
# libmpv is taken from ~/zlibmpv/libmpv.2.dylib (the workflow's cache) when it is there; the app plays nothing
# without it. Output: windows/app/dist/zunflix.pkg and windows/app/dist/shots.
set -euo pipefail
cd "$(dirname "$0")/../windows/app"
export MACOSX_DEPLOYMENT_TARGET=12.0
# (a dev build: quicker to link than a release, the same code)
export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16

rustup target add aarch64-apple-darwin x86_64-apple-darwin

cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

mkdir -p dist/libmpv
if [ -f "$HOME/zlibmpv/libmpv.2.dylib" ]; then
  cp "$HOME/zlibmpv/libmpv.2.dylib" dist/libmpv/
  lipo -archs dist/libmpv/libmpv.2.dylib
else
  echo "warning: no libmpv yet: the app plays nothing"
fi

# (the Mac's own version and build, as net.rs has them: released apart from Windows')
VERSION=$(grep -oE 'MAC_NAME: &str = "[^"]+"' src/net.rs | cut -d'"' -f2)
BUILD=$(grep -oE 'MAC_BUILD: i64 = [0-9]+' src/net.rs | grep -oE '[0-9]+$')
APP=dist/zunflix.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/assets"
lipo -create -output "$APP/Contents/MacOS/zunflix" \
  target/aarch64-apple-darwin/release/zunflix target/x86_64-apple-darwin/release/zunflix
lipo -info "$APP/Contents/MacOS/zunflix"
cp -R assets/fonts "$APP/Contents/Resources/assets/fonts"
cp assets/icon.ico "$APP/Contents/Resources/assets/icon.ico"
# libmpv where the Loader looks first (Contents/Frameworks); its install name is @rpath/libmpv.2.dylib
if [ -f dist/libmpv/libmpv.2.dylib ]; then
  mkdir -p "$APP/Contents/Frameworks"
  cp dist/libmpv/libmpv.2.dylib "$APP/Contents/Frameworks/"
fi
# the icon: the .ico's biggest picture (a PNG inside it) at every size an .icns holds
ICONSET=dist/zunflix.iconset
mkdir -p "$ICONSET"
python3 - <<'PY'
import struct
ico = open("assets/icon.ico", "rb").read()
pictures = [struct.unpack("<II", ico[14 + i * 16:22 + i * 16]) for i in range(struct.unpack("<H", ico[4:6])[0])]
size, at = max(pictures)
open("dist/icon.png", "wb").write(ico[at:at + size])
PY
for size in 16 32 128 256 512; do
  sips -z $size $size dist/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) dist/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/zunflix.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>cc.zunflix.app</string>
  <key>CFBundleName</key><string>zunflix</string>
  <key>CFBundleDisplayName</key><string>zunflix</string>
  <key>CFBundleExecutable</key><string>zunflix</string>
  <key>CFBundleIconFile</key><string>zunflix</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${BUILD}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.entertainment</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist"
# Signed with the Developer ID (MAC_P12, MAC_P12_PASSWORD) when it's given, under the hardened runtime notarizing needs:
# libmpv first, then the app (Apple: sign inside out, never --deep). Without it, ad hoc: unsigned for anyone else, but
# Apple silicon runs nothing without a signature.
TMP=${RUNNER_TEMP:-$(mktemp -d)}
ID=-
if [ -n "${MAC_P12:-}" ]; then
  # (GitHub's documented way: a keychain of its own for the build, unlocked, searched first)
  KC="$TMP/signing.keychain-db"
  KCP=$(openssl rand -hex 16)
  printf %s "$MAC_P12" | base64 --decode > "$TMP/signing.p12"
  security create-keychain -p "$KCP" "$KC"
  security set-keychain-settings -lut 21600 "$KC"
  security unlock-keychain -p "$KCP" "$KC"
  security import "$TMP/signing.p12" -P "$MAC_P12_PASSWORD" -A -t cert -f pkcs12 -k "$KC"
  security set-key-partition-list -S apple-tool:,apple: -k "$KCP" "$KC" >/dev/null
  security list-keychain -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
  rm -f "$TMP/signing.p12"
  ID=$(security find-identity -v -p codesigning "$KC" | grep -o '"Developer ID Application[^"]*"' | head -n 1 | tr -d '"')
  [ -n "$ID" ] || { echo "no Developer ID identity in the certificate"; exit 1; }
  if [ -d "$APP/Contents/Frameworks" ]; then
    find "$APP/Contents/Frameworks" -name '*.dylib' -exec codesign --force --timestamp --options runtime -s "$ID" {} \;
  fi
  codesign --force --timestamp --options runtime -s "$ID" "$APP"
  codesign --verify --strict --verbose=2 "$APP"
else
  codesign --force --deep -s - "$APP"
fi

rm -rf dist/notarized dist/zunflix.pkg dist/pkgroot dist/pkg-scripts
# The installer package: zunflix.app into Applications, nothing to drag. Its postinstall hands the app to the user
# signed in (so the app's own updates swap it in without a password) and opens it.
mkdir -p dist/pkgroot/Applications dist/pkg-scripts
ditto dist/zunflix.app dist/pkgroot/Applications/zunflix.app
cat > dist/pkg-scripts/postinstall <<'POST'
#!/bin/sh
app="${2%/}/Applications/zunflix.app"
user=$(stat -f%Su /dev/console)
if [ -n "$user" ] && [ "$user" != root ]; then
  chown -R "$user":staff "$app"
  launchctl asuser "$(id -u "$user")" sudo -u "$user" open "$app" || true
fi
exit 0
POST
chmod +x dist/pkg-scripts/postinstall
# (not relocatable: Installer puts it in Applications, never "updates" a stray copy found elsewhere)
pkgbuild --analyze --root dist/pkgroot dist/pkg-components.plist
plutil -replace 0.BundleIsRelocatable -bool NO dist/pkg-components.plist
# Signed with the Developer ID Installer certificate (MAC_INSTALLER_P12; its password MAC_INSTALLER_P12_PASSWORD, else
# MAC_P12_PASSWORD), a different one from the app's: a package can't be signed with the Application one.
IID=
if [ "$ID" != "-" ] && [ -n "${MAC_INSTALLER_P12:-}" ]; then
  printf %s "$MAC_INSTALLER_P12" | base64 --decode > "$TMP/installer.p12"
  security import "$TMP/installer.p12" -P "${MAC_INSTALLER_P12_PASSWORD:-$MAC_P12_PASSWORD}" -A -t cert -f pkcs12 -k "$KC"
  security set-key-partition-list -S apple-tool:,apple: -k "$KCP" "$KC" >/dev/null
  rm -f "$TMP/installer.p12"
  IID=$(security find-identity -v "$KC" | grep -o '"Developer ID Installer[^"]*"' | head -n 1 | tr -d '"')
  [ -n "$IID" ] || { echo "no Developer ID Installer identity in its certificate"; exit 1; }
fi
if [ -n "$IID" ]; then
  pkgbuild --root dist/pkgroot --component-plist dist/pkg-components.plist --scripts dist/pkg-scripts \
    --identifier cc.zunflix.app --version "$VERSION" --install-location / --sign "$IID" --timestamp dist/zunflix.pkg
else
  pkgbuild --root dist/pkgroot --component-plist dist/pkg-components.plist --scripts dist/pkg-scripts \
    --identifier cc.zunflix.app --version "$VERSION" --install-location / dist/zunflix.pkg
fi
rm -rf dist/pkgroot dist/pkg-scripts dist/pkg-components.plist
# Notarized (ASC_KEY_P8, ASC_KEY_ID, ASC_ISSUER_ID: an App Store Connect API key) and the ticket stapled on, so it opens
# on any Mac without the unidentified-developer warning, offline too
if [ -n "$IID" ] && [ -n "${ASC_KEY_P8:-}" ]; then
  printf %s "$ASC_KEY_P8" | base64 --decode > "$TMP/notary.p8"
  # (Apple took over 45 minutes on the first one: waited on up to 3 hours. Still waiting then isn't a failure: the
  # signed .pkg is the file Apple is looking at, and a Mac finds Apple's yes online once it's given)
  out=$(xcrun notarytool submit dist/zunflix.pkg --key "$TMP/notary.p8" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" --wait --timeout 3h 2>&1) || true
  echo "$out" | grep -v "Current status: In Progress" || true
  if echo "$out" | grep -q "status: Accepted"; then
    xcrun stapler staple dist/zunflix.pkg
    spctl -a -t install -vv dist/zunflix.pkg
    # (what tools/release.sh mac looks for: only a notarized, stapled .pkg is ever released)
    touch dist/notarized
  elif echo "$out" | grep -qE "status: (Invalid|Rejected)"; then
    sub=$(echo "$out" | awk '/^ *id:/{print $2; exit}')
    [ -n "$sub" ] && xcrun notarytool log "$sub" --key "$TMP/notary.p8" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" || true
    rm -f "$TMP/notary.p8"
    echo "notarizing failed"; exit 1
  else
    echo "warning: Apple hasn't answered yet; the .pkg is signed, unstapled"
  fi
  rm -f "$TMP/notary.p8"
elif [ "$ID" != "-" ]; then
  echo "warning: no Developer ID Installer certificate (MAC_INSTALLER_P12): the .pkg is unsigned, not notarized"
fi

set +e
mkdir -p dist/shots
DATA="$HOME/Library/Application Support/zunflix"
shot() { screencapture -x "dist/shots/$1.png"; }
# Signed in as the test account when its secrets are there: Who's watching, the profile, Home, then the hero's Play.
# (the clicks are in screen points on the runner's 1024x768 display)
BASE=$(grep -oE 'pub const BASE: &str = "[^"]+"' ../../core/zcore/src/api.rs | cut -d'"' -f2)
SIGNED_IN=
if [ -n "${ZF_TEST_USER:-}" ] && python3 ../../tools/mac_signin.py "$BASE" "$DATA"; then
  SIGNED_IN=1
  command -v cliclick >/dev/null || brew install -q cliclick
fi
# (H.264 and SDR only: a virtual Mac has no graphics card to draw HEVC or HDR with)
ZUNFLIX_CODECS=h264,ac3,eac3,dts,truehd,pcm,opus,flac,local,file ./dist/zunflix.app/Contents/MacOS/zunflix > dist/shots/app.log 2>&1 &
APP_PID=$!
for i in 1 2 3 4 5 6; do sleep 1.5; shot 0-early-$i; done
sleep 1; shot 1-start
sleep 3; shot 2-after-10s
if [ -n "$SIGNED_IN" ]; then
  cliclick c:465,360; sleep 6; shot 3-profile-picked
  sleep 6; shot 4-home
  cliclick c:180,461; sleep 5; shot 5-after-play
  # (a show's Play opens its page: its Resume starts the episode)
  cliclick c:156,400; sleep 5; shot 5-play-5s
  sleep 10; shot 6-play-15s
  sleep 15; shot 7-play-30s
  cliclick m:600,300; sleep 1; shot 8-controls
else
  sleep 20; shot 3-at-30s
fi
kill -0 $APP_PID && echo "still running" || echo "the app had quit"
kill $APP_PID 2>/dev/null
[ -n "$SIGNED_IN" ] && python3 ../../tools/mac_signin.py "$BASE" "$DATA" --out
cp "$DATA/errors.txt" "$DATA/crash.txt" dist/shots/ 2>/dev/null
ls -la dist/shots
exit 0
