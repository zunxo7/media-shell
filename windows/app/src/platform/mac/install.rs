//! zunflix on a Mac: a .app bundle put in Applications by its installer package (.pkg; older builds came as a .dmg),
//! so there's no setup of its own; the app's folders are where macOS keeps an app's things, and uninstalling moves the
//! bundle to the Trash.
use objc2_foundation::{NSBundle, NSFileManager, NSURL};
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// The bundle this app runs from, when it runs from one.
fn bundle() -> Option<PathBuf> {
    let path = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string());
    path.extension().is_some_and(|e| e == "app").then_some(path)
}

/// A setup is a Windows thing: a Mac build is packed as a .pkg (tools/build-mac-app.sh).
pub fn make(_stage: &str, _out: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("a Mac build is packed as a .pkg (tools/build-mac-app.sh)"))
}

/// Never: the app is put in place by its installer package.
pub fn is_setup() -> bool {
    false
}

/// How big the app is unpacked: nothing to unpack on a Mac.
pub fn size() -> u64 {
    0
}

/// The setup's own fonts: the bundle always has them.
pub fn own_assets() -> Option<PathBuf> {
    None
}

/// The fonts, icons and pictures: in the bundle's Resources, or beside the binary run on its own.
pub fn assets_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().unwrap_or(Path::new("."));
    let resources = dir.join("../Resources/assets");
    if resources.is_dir() {
        resources
    } else {
        dir.join("assets")
    }
}

/// Where zunflix keeps its things on this Mac (sign-in, settings, downloads), without making the folder.
pub fn data_dir() -> PathBuf {
    home().join("Library/Application Support/zunflix")
}

/// Where zunflix keeps what it can fetch again (pictures, pages): the Caches folder macOS may clear.
pub fn cache_dir() -> PathBuf {
    home().join("Library/Caches/zunflix")
}

/// Never: a Mac update is put in place by a helper once the app has closed (start_update), not by another app.
pub fn updating() -> Option<u32> {
    None
}

/// The team every zunflix build is signed by: an update signed by anyone else is never put in place.
const TEAM: &str = "RJT2PYF6R6";

/// Where an update's new bundle is unpacked: beside this one, so it's swapped in by a rename.
fn staged(app: &Path) -> PathBuf {
    PathBuf::from(format!("{}.new", app.display()))
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The first zunflix.app under `dir` (a package's payload nests it a few folders down).
fn find_app(dir: &Path, depth: u32) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "zunflix.app") && path.is_dir() {
            return Some(path);
        }
        if depth > 0 && path.is_dir() {
            if let Some(found) = find_app(&path, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Whether `file` starts with `magic`: what kind of file the core fetched, whatever its name.
fn starts_with(file: &Path, magic: &[u8]) -> bool {
    let mut head = vec![0u8; magic.len()];
    std::fs::File::open(file).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok() && head == magic
}

/// Whether the app's bundle can be replaced by this user: its folder is theirs to write (not /Applications owned by root
/// with a user who isn't an administrator).
fn replaceable(app: &Path) -> bool {
    let Some(dir) = app.parent() else { return false };
    let probe = dir.join(".zunflix-write-test");
    let wrote = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    wrote
}

/// Whether the package is signed with a Developer ID Installer certificate of TEAM: Installer opens an unsigned one
/// fetched by the app without a word (it isn't quarantined), so it's checked here first.
fn package_ours(file: &Path) -> bool {
    std::process::Command::new("/usr/sbin/pkgutil").arg("--check-signature").arg(file).output().is_ok_and(|o| {
        let text = String::from_utf8_lossy(&o.stdout);
        o.status.success() && text.lines().any(|l| l.contains("Developer ID Installer:") && l.trim_end().ends_with(&format!("({TEAM})")))
    })
}

/// Whether the bundle at `app` is intact and signed by TEAM.
fn ours(app: &Path) -> bool {
    run("/usr/bin/codesign", &["--verify".as_ref(), "--deep".as_ref(), "--strict".as_ref(), app.as_os_str()])
        && std::process::Command::new("/usr/bin/codesign")
            .arg("-dv")
            .arg(app)
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stderr).lines().any(|l| l == format!("TeamIdentifier={TEAM}")))
}

/// The update downloaded to `file` (the app as a zip, or the installer package, or an older build's .dmg) made ready
/// while the app is still open, as the last part of the update's bar (`progress`, 0 to 1): its bundle unpacked beside
/// this one and checked as ours (signed by TEAM, intact). False when it can't be (no bundle to replace, a folder that
/// can't be written, a bundle that isn't ours): the core is then told, and it chooses the installer package, or says
/// it couldn't. The one exception is a package that can't be swapped in by the user: it is left for the installer, which
/// asks for a password (`start_update` opens it), so the app is never left without a way.
pub fn prepare_update(file: &Path, progress: impl Fn(f32)) -> bool {
    let Some(app) = bundle() else { return false };
    let is_zip = starts_with(file, b"PK");
    let is_pkg = starts_with(file, b"xar!");
    if !replaceable(&app) {
        return is_pkg;
    }
    let new = staged(&app);
    let _ = std::fs::remove_dir_all(&new);
    progress(0.1);
    let unpacked = if is_zip {
        // (unpacked beside the bundle, on its volume, so it is moved in by a rename, not copied again)
        let work = PathBuf::from(format!("{}.unzip", app.display()));
        let _ = std::fs::remove_dir_all(&work);
        let unzipped = std::fs::create_dir_all(&work).is_ok()
            && run("/usr/bin/ditto", &["-x".as_ref(), "-k".as_ref(), file.as_os_str(), work.as_os_str()])
            && find_app(&work, 1).is_some_and(|found| {
                progress(0.6);
                std::fs::rename(found, &new).is_ok()
            });
        let _ = std::fs::remove_dir_all(&work);
        unzipped
    } else {
        let work = std::env::temp_dir().join(format!("zunflix-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let unpacked = if is_pkg {
            run("/usr/sbin/pkgutil", &["--expand-full".as_ref(), file.as_os_str(), work.as_os_str()])
                && find_app(&work, 5).is_some_and(|found| {
                    progress(0.6);
                    run("/usr/bin/ditto", &[found.as_os_str(), new.as_os_str()])
                })
        } else {
            let mnt = work.join("mnt");
            let _ = std::fs::create_dir_all(&mnt);
            let attached = run(
                "/usr/bin/hdiutil",
                &["attach".as_ref(), "-nobrowse".as_ref(), "-noautoopen".as_ref(), "-quiet".as_ref(), "-mountpoint".as_ref(), mnt.as_os_str(), file.as_os_str()],
            );
            progress(0.6);
            let copied = attached && run("/usr/bin/ditto", &[mnt.join("zunflix.app").as_os_str(), new.as_os_str()]);
            if attached {
                run("/usr/bin/hdiutil", &["detach".as_ref(), "-quiet".as_ref(), mnt.as_os_str()]);
            }
            copied
        };
        let _ = std::fs::remove_dir_all(&work);
        unpacked
    };
    progress(0.8);
    // (the file itself stays: the core checked it, and clears the updates folder once it's the newest)
    if !(unpacked && ours(&new)) {
        let _ = std::fs::remove_dir_all(&new);
        return false;
    }
    progress(1.);
    true
}

/// The update `prepare_update` made ready, put in place once this app has closed: a helper (a shell script, in a process
/// group of its own, so it outlives this app) waits for it to go, moves the old bundle aside, moves the new one in,
/// clears its quarantine and opens it, putting the old one back if the swap fails. With no bundle staged (a package a
/// user without the rights can't swap in) the installer opens the package instead.
pub fn start_update(file: &Path) -> bool {
    use std::os::unix::process::CommandExt;
    let Some(app) = bundle() else { return false };
    if !staged(&app).is_dir() {
        return starts_with(file, b"xar!")
            && package_ours(file)
            && std::process::Command::new("/usr/bin/open").arg("-a").arg("Installer").arg(file).status().is_ok_and(|s| s.success());
    }
    const HELPER: &str = r#"
pid="$1"; app="$2"
n=0
while kill -0 "$pid" 2>/dev/null; do
  n=$((n + 1)); [ "$n" -gt 300 ] && { rm -rf "$app.new"; exit 1; }
  sleep 0.2
done
rm -rf "$app.old"
if mv "$app" "$app.old"; then
  if mv "$app.new" "$app"; then
    xattr -dr com.apple.quarantine "$app" 2>/dev/null
    rm -rf "$app.old"
  else
    mv "$app.old" "$app"
  fi
fi
rm -rf "$app.new"
open "$app"
"#;
    std::process::Command::new("/bin/sh")
        .args(["-c", HELPER, "zunflix-update", &std::process::id().to_string()])
        .arg(&app)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

/// Run straight from its .dmg (as people do, skipping the drag), it offers to move itself into Applications, as
/// Firefox and Slack do: copied there, opened from there, the .dmg let go. True: this run hands over and ends.
pub fn place() -> bool {
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication};
    use objc2_foundation::{MainThreadMarker, NSString};
    let Some(app) = bundle().filter(|b| b.starts_with("/Volumes/")) else { return false };
    let Some(mtm) = MainThreadMarker::new() else { return false };
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(zcore::setup::MOVE_TITLE));
    alert.setInformativeText(&NSString::from_str(zcore::setup::MOVE_LINE));
    alert.addButtonWithTitle(&NSString::from_str(zcore::setup::MOVE_BUTTON));
    alert.addButtonWithTitle(&NSString::from_str(zcore::setup::MOVE_LATER));
    if alert.runModal() != NSAlertFirstButtonReturn {
        return false;
    }
    let target = PathBuf::from("/Applications/zunflix.app");
    let _ = std::fs::remove_dir_all(&target);
    let copied = std::process::Command::new("/usr/bin/ditto").arg(&app).arg(&target).status().is_ok_and(|s| s.success());
    if !copied {
        return false;
    }
    // (the .dmg let go once this run has ended: the new copy opens from Applications)
    let volume: PathBuf = app.components().take(3).collect();
    let _ = std::process::Command::new("/bin/sh")
        .args(["-c", r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; open "$2"; hdiutil detach -quiet "$3""#, "zunflix-place"])
        .arg(std::process::id().to_string())
        .arg(&target)
        .arg(&volume)
        .spawn();
    true
}

/// The binary can be run from inside the bundle being trashed: nothing to copy first.
pub fn relaunch_from_temp(_args: &[String]) -> bool {
    false
}

/// Taking zunflix off this Mac, telling `done` how many files of how many are gone: once the app has closed, what
/// it kept here, all but downloaded films and episodes, then the bundle to the Trash.
pub fn uninstall(done: impl Fn(usize, usize)) {
    for pid in std::env::args().filter_map(|a| a.strip_prefix("--wait=").and_then(|p| p.parse::<u32>().ok())) {
        for _ in 0..150 {
            let alive = std::process::Command::new("/bin/kill").args(["-0", &pid.to_string()]).status().is_ok_and(|s| s.success());
            if !alive {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out); } else { out.push(p); }
            }
        }
    }
    let (data, cache) = (data_dir(), cache_dir());
    let downloads = data.join("Downloads");
    let keep = std::fs::read_dir(&downloads).map(|mut d| d.next().is_some()).unwrap_or(false);
    let mut files = Vec::new();
    walk(&data, &mut files);
    walk(&cache, &mut files);
    if keep {
        files.retain(|f| !f.starts_with(&downloads));
    }
    let all = files.len();
    for (i, f) in files.iter().enumerate() {
        let _ = std::fs::remove_file(f);
        if i % 8 == 0 || i + 1 == all { done(i + 1, all); }
    }
    let _ = std::fs::remove_dir_all(&cache);
    if keep {
        if let Ok(entries) = std::fs::read_dir(&data) {
            for e in entries.flatten() {
                if e.path() != downloads { let _ = std::fs::remove_dir_all(e.path()); }
            }
        }
    } else {
        let _ = std::fs::remove_dir_all(&data);
    }
    if let Some(app) = bundle() {
        let url = NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(&app.to_string_lossy()));
        let _ = NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None);
    }
    done(all, all);
}

/// Nothing is left behind to clean up: the uninstaller ran from the bundle itself.
pub fn clean_up_after_uninstall() {}

/// Updates aren't installed by the app yet: nothing to do.
pub fn install_silently(_pid: Option<u32>) {}

/// The app is put in place by its installer package, never by itself.
pub fn install(_pid: Option<u32>, _done: impl Fn(f32)) -> std::result::Result<PathBuf, String> {
    Err("zunflix is installed by opening its installer package.".into())
}

/// Uninstalling from Settings: the uninstaller opens (already said yes to) and waits for zunflix to close.
pub fn uninstall_from_app() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(["--uninstall", "--confirmed", &format!("--wait={}", std::process::id())]).spawn();
    }
}
