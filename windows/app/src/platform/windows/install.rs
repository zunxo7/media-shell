//! zunflix's own setup on Windows: the app's files packed (xz) onto the end of a copy of zunflix.exe. Run like that,
//! the app is its own installer, no admin and no third-party installer; `/S /UPDATE-PID=<pid>` installs without a
//! window once that app has closed (updates). Made by `zunflix.exe --make-setup <files> <setup.exe>`
//! (installer/build.ps1), taken away by `--uninstall`.
use crate::platform::Result;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use windows::core::*;

const MAGIC: &[u8; 8] = b"ZFSETUP1";
const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ZunFlix";

// ---------- the pack ----------
/// Writes `out`: this exe, then every file under `stage` (fonts first, so the setup can show itself before the rest
/// is out), packed, then where the pack starts and how big it is unpacked.
pub fn make(stage: &str, out: &str) -> std::io::Result<()> {
    let me = std::fs::read(std::env::current_exe()?)?;
    let mut files = Vec::new();
    fn walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() { walk(root, &p, files)?; } else { files.push(p.strip_prefix(root).unwrap().to_path_buf()); }
        }
        Ok(())
    }
    let root = Path::new(stage);
    walk(root, root, &mut files)?;
    // (not the app itself: the setup is the app, and puts a copy of itself in place, with the app's icon)
    files.retain(|f| f.to_string_lossy() != "zunflix.exe");
    files.sort_by_key(|p| (!p.to_string_lossy().replace('\\', "/").starts_with("assets/"), p.clone()));
    // (xz at its strongest, after the x86 filter that makes machine code pack tighter: libmpv, most of it, packs
    // a third smaller than deflate's best)
    let mut filters = xz2::stream::Filters::new();
    filters.x86();
    filters.lzma2(&xz2::stream::LzmaOptions::new_preset(9 | 0x8000_0000).map_err(std::io::Error::other)?);
    let stream = xz2::stream::Stream::new_stream_encoder(&filters, xz2::stream::Check::Crc64).map_err(std::io::Error::other)?;
    let mut packed = xz2::write::XzEncoder::new_stream(Vec::new(), stream);
    let mut total = 0u64;
    for f in &files {
        let name = f.to_string_lossy().replace('\\', "/");
        let data = std::fs::read(root.join(f))?;
        packed.write_all(&(name.len() as u16).to_le_bytes())?;
        packed.write_all(name.as_bytes())?;
        packed.write_all(&(data.len() as u64).to_le_bytes())?;
        packed.write_all(&data)?;
        total += data.len() as u64;
    }
    packed.write_all(&0u16.to_le_bytes())?;
    let packed = packed.finish()?;
    // this exe, with the setup's own icon (zf over "setup") in place of the app's, then the pack
    std::fs::write(out, &me)?;
    set_icon(out, include_bytes!("../../../assets/setup.ico")).map_err(|e| std::io::Error::other(e.to_string()))?;
    let start = std::fs::metadata(out)?.len();
    let mut o = std::fs::OpenOptions::new().append(true).open(out)?;
    o.write_all(&packed)?;
    o.write_all(&start.to_le_bytes())?;
    o.write_all(&total.to_le_bytes())?;
    o.write_all(MAGIC)?;
    Ok(())
}

/// Puts an .ico's pictures into an exe as its icon (group 1, its pictures 1..n, neutral language: as zunflix.rc has
/// the app's), in place of what's there.
fn set_icon(exe: &str, ico: &[u8]) -> Result<()> {
    use windows::Win32::System::LibraryLoader::*;
    let word = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let long = |at: usize| u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]);
    let count = word(4) as usize;
    let mut group = vec![0u8, 0, 1, 0];
    group.extend_from_slice(&(count as u16).to_le_bytes());
    unsafe {
        let update = BeginUpdateResourceW(&HSTRING::from(exe), false)?;
        for i in 0..count {
            let e = 6 + i * 16;
            let (size, at) = (long(e + 8) as usize, long(e + 12) as usize);
            let picture = &ico[at..at + size];
            UpdateResourceW(update, PCWSTR(3 as _), PCWSTR((i + 1) as _), 0, Some(picture.as_ptr() as *const _), size as u32)?;
            group.extend_from_slice(&ico[e..e + 12]);
            group.extend_from_slice(&((i + 1) as u16).to_le_bytes());
        }
        UpdateResourceW(update, PCWSTR(14 as _), PCWSTR(1 as _), 0, Some(group.as_ptr() as *const _), group.len() as u32)?;
        EndUpdateResourceW(update, false)?;
    }
    Ok(())
}

/// Where this exe's pack starts and how big it is unpacked: Some when it's a setup.
fn payload() -> Option<(u64, u64)> {
    let mut f = std::fs::File::open(std::env::current_exe().ok()?).ok()?;
    let len = f.seek(SeekFrom::End(0)).ok()?;
    if len < 24 { return None; }
    f.seek(SeekFrom::End(-24)).ok()?;
    let mut tail = [0u8; 24];
    f.read_exact(&mut tail).ok()?;
    (&tail[16..] == MAGIC).then(|| (u64::from_le_bytes(tail[..8].try_into().unwrap()), u64::from_le_bytes(tail[8..16].try_into().unwrap())))
}

/// Whether this exe is a setup (the app's files packed onto it).
pub fn is_setup() -> bool {
    payload().is_some()
}

/// How big the app is unpacked, in bytes (0 when this isn't a setup).
pub fn size() -> u64 {
    payload().map_or(0, |(_, t)| t)
}

/// The pack's files out to `dest` (those `want` takes), naming each one written, telling `done` how far (0 to 1).
fn unpack(dest: &Path, want: impl Fn(&str) -> bool, done: impl Fn(f32)) -> std::io::Result<Vec<String>> {
    let (at, total) = payload().ok_or_else(|| std::io::Error::other("not a setup"))?;
    let mut f = std::fs::File::open(std::env::current_exe()?)?;
    f.seek(SeekFrom::Start(at))?;
    let mut z = xz2::read::XzDecoder::new(std::io::BufReader::new(f));
    let mut seen = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    let mut written = Vec::new();
    loop {
        let mut n = [0u8; 2];
        z.read_exact(&mut n)?;
        let n = u16::from_le_bytes(n) as usize;
        if n == 0 { break; }
        let mut name = vec![0u8; n];
        z.read_exact(&mut name)?;
        let name = String::from_utf8_lossy(&name).to_string();
        let mut size = [0u8; 8];
        z.read_exact(&mut size)?;
        let mut left = u64::from_le_bytes(size);
        let mut out = if want(&name) {
            let path = dest.join(&name);
            if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
            written.push(name.clone());
            Some(std::fs::File::create(&path)?)
        } else {
            None
        };
        while left > 0 {
            let take = left.min(buf.len() as u64) as usize;
            z.read_exact(&mut buf[..take])?;
            if let Some(o) = out.as_mut() { o.write_all(&buf[..take])?; }
            left -= take as u64;
            seen += take as u64;
            done((seen as f32 / total.max(1) as f32).min(1.));
        }
    }
    Ok(written)
}

/// Everything in the app's folder that isn't this version's (`kept`, as the pack names them): what older versions
/// left, the Java app's ~100 MB runtime among them. The viewer's data lives elsewhere (`data_dir`).
fn prune(dir: &Path, kept: &std::collections::HashSet<String>, at: &Path) {
    let Ok(entries) = std::fs::read_dir(at) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.strip_prefix(dir).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        if path.is_dir() {
            prune(dir, kept, &path);
            let _ = std::fs::remove_dir(&path);
        } else if !kept.contains(&name) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// The fonts and icon, out to a folder of their own, so the setup can draw itself before anything is installed.
pub fn own_assets() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("zunflix-setup");
    let _ = std::fs::create_dir_all(&dir);
    // (only what's under assets/: the pack puts it first, so this stops reading early... the rest is skipped by name)
    unpack(&dir, |name| name.starts_with("assets/fonts/") || name == "assets/icon.ico", |_| {}).ok()?;
    Some(dir.join("assets").join("fonts"))
}

/// The fonts, icons and pictures, beside the exe.
pub fn assets_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("assets")
}

fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("Programs").join("ZunFlix")
}

/// Where zunflix keeps its things on this PC (sign-in, settings, pictures, downloads): as net::root has it, without
/// making the folder.
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("ZunFlix")
}

/// Where zunflix keeps what it can fetch again (pictures, pages): inside its data folder.
pub fn cache_dir() -> PathBuf {
    data_dir().join("Cache")
}

/// The app this install is updating (it opened us to put a new version in place, then closed).
pub fn updating() -> Option<u32> {
    std::env::args().find_map(|a| a.strip_prefix("/UPDATE-PID=").and_then(|p| p.parse().ok()))
}

/// Never: the setup puts zunflix in its place (the Mac's own, run from its .dmg, offers to move itself).
pub fn place() -> bool {
    false
}

/// The update downloaded to `setup` started, told to take over from this app once it's closed.
/// Nothing to make ready here: the setup does all of it, in its own window, once this app has closed.
pub fn prepare_update(_file: &Path, _progress: impl Fn(f32)) -> bool {
    true
}

pub fn start_update(setup: &Path) -> bool {
    std::process::Command::new(setup).arg(format!("/UPDATE-PID={}", std::process::id())).spawn().is_ok()
}

/// The uninstaller can't take away the exe it runs from: so, run from zunflix's folder, it copies itself (with its
/// fonts and icon) to a folder in Temp and starts that copy, which does the work. True: the copy's started.
pub fn relaunch_from_temp(args: &[String]) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let temp = std::env::temp_dir().join("zunflix-uninstall");
    if exe.starts_with(&temp) {
        return false;
    }
    let copy = || -> std::io::Result<PathBuf> {
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("assets").join("fonts"))?;
        let to = temp.join("zunflix.exe");
        std::fs::copy(&exe, &to)?;
        let from = exe.parent().unwrap_or(Path::new("."));
        for f in std::fs::read_dir(from.join("assets").join("fonts"))?.flatten() {
            std::fs::copy(f.path(), temp.join("assets").join("fonts").join(f.file_name()))?;
        }
        let _ = std::fs::copy(from.join("assets").join("icon.ico"), temp.join("assets").join("icon.ico"));
        let _ = std::fs::copy(from.join("vcruntime140.dll"), temp.join("vcruntime140.dll"));
        Ok(to)
    };
    let Ok(to) = copy() else { return false };
    let mut rest: Vec<String> = args.iter().skip(1).cloned().collect();
    rest.push(format!("--wait={}", std::process::id()));
    std::process::Command::new(to).args(rest).current_dir(&temp).spawn().is_ok()
}

/// Waits (up to `ms`) for the process `pid` to end.
fn wait_for(pid: u32, ms: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::*;
    unsafe {
        if let Ok(p) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(p, ms);
            let _ = CloseHandle(p);
        }
    }
}

/// Taking zunflix off this PC, telling `done` how many files of how many are gone: once the app (and the copy that
/// started this) have closed, the shortcuts, the entry in installed apps, zunflix's folder, and what it kept here, all
/// but downloaded films and episodes.
pub fn uninstall(done: impl Fn(usize, usize)) {
    use windows::Win32::System::Registry::*;
    for pid in std::env::args().filter_map(|a| a.strip_prefix("--wait=").and_then(|p| p.parse::<u32>().ok())) {
        wait_for(pid, 30_000);
    }
    for folder in [windows::Win32::UI::Shell::FOLDERID_Programs, windows::Win32::UI::Shell::FOLDERID_Desktop] {
        if let Some(dir) = known_folder(&folder) {
            let _ = std::fs::remove_file(dir.join("zunflix.lnk"));
            let _ = std::fs::remove_file(dir.join("ZunFlix.lnk"));
        }
    }
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(KEY));
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out); } else { out.push(p); }
            }
        }
    }
    let (app_dir, data) = (install_dir(), data_dir());
    let downloads = data.join("Downloads");
    let keep = std::fs::read_dir(&downloads).map(|mut d| d.next().is_some()).unwrap_or(false);
    let mut files = Vec::new();
    walk(&app_dir, &mut files);
    walk(&data, &mut files);
    if keep {
        files.retain(|f| !f.starts_with(&downloads));
    }
    let all = files.len();
    for (i, f) in files.iter().enumerate() {
        for _ in 0..10 {
            if std::fs::remove_file(f).is_ok() || !f.exists() { break; }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        if i % 8 == 0 || i + 1 == all { done(i + 1, all); }
    }
    let _ = std::fs::remove_dir_all(&app_dir);
    if keep {
        if let Ok(entries) = std::fs::read_dir(&data) {
            for e in entries.flatten() {
                if e.path() != downloads { let _ = std::fs::remove_dir_all(e.path()); }
            }
        }
    } else {
        let _ = std::fs::remove_dir_all(&data);
    }
    done(all, all);
}

/// The uninstaller's own folder in Temp taken away a moment after it has closed.
pub fn clean_up_after_uninstall() {
    let temp = std::env::temp_dir().join("zunflix-uninstall");
    let _ = std::process::Command::new("cmd")
        .args(["/c", &format!("ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"", temp.to_string_lossy())])
        .spawn();
}

/// An update's install: no window; once the app that asked for it has closed, the files, then zunflix again.
pub fn install_silently(pid: Option<u32>) {
    if let Ok(exe) = install(pid, |_| {}) {
        let _ = std::process::Command::new(exe).spawn();
    }
}

/// Puts the app in place: waits for `pid` (the app being updated) to close, closes a zunflix still open, unpacks,
/// and makes the shortcuts and the entry in Windows' installed apps. The installed exe.
pub fn install(pid: Option<u32>, done: impl Fn(f32)) -> std::result::Result<PathBuf, String> {
    if let Some(pid) = pid {
        wait_for(pid, 120_000);
    }
    // (a zunflix left open: asked to close, so its files can be replaced)
    super::window::close_running_app();
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| zcore::setup::folder_failed(&e.to_string()))?;
    let mut last = None;
    let mut kept = std::collections::HashSet::new();
    for _ in 0..10 {
        match unpack(&dir, |_| true, &done) {
            Ok(names) => { last = None; kept.extend(names); break; }
            Err(e) => { last = Some(e); std::thread::sleep(std::time::Duration::from_millis(500)); }
        }
    }
    if let Some(e) = last {
        return Err(zcore::setup::place_failed(&e.to_string()));
    }
    kept.insert("zunflix.exe".into());
    prune(&dir, &kept, &dir);
    let exe = dir.join("zunflix.exe");
    let icon = dir.join("assets").join("icon.ico");
    // the app: this setup without its pack (it is the app), given back the app's own icon
    let me = std::fs::read(std::env::current_exe().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let at = payload().map_or(me.len() as u64, |(at, _)| at) as usize;
    let mut placed = Err(String::new());
    for _ in 0..10 {
        placed = std::fs::write(&exe, &me[..at]).map_err(|e| e.to_string());
        if placed.is_ok() { break; }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    placed.map_err(|e| zcore::setup::place_failed(&e))?;
    if let Ok(ico) = std::fs::read(&icon) {
        let _ = set_icon(&exe.to_string_lossy(), &ico);
    }
    let _ = shortcuts(&exe, &icon);
    let _ = register(&exe, &icon, &dir);
    // (Windows told its icons changed: the taskbar and shortcuts show the new one, not the one it kept from before)
    unsafe { windows::Win32::UI::Shell::SHChangeNotify(windows::Win32::UI::Shell::SHCNE_ASSOCCHANGED, windows::Win32::UI::Shell::SHCNF_IDLIST, None, None); }
    Ok(exe)
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::*;
    unsafe {
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let path = PathBuf::from(p.to_string().ok()?);
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        Some(path)
    }
}

fn shortcuts(exe: &Path, icon: &Path) -> Result<()> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for folder in [FOLDERID_Programs, FOLDERID_Desktop] {
            let Some(dir) = known_folder(&folder) else { continue };
            let _ = std::fs::remove_file(dir.join("ZunFlix.lnk"));
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe.as_os_str()))?;
            link.SetWorkingDirectory(&HSTRING::from(exe.parent().unwrap_or(exe).as_os_str()))?;
            link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
            link.SetDescription(w!("zunflix"))?;
            let file: IPersistFile = link.cast()?;
            file.Save(&HSTRING::from(dir.join("zunflix.lnk").as_os_str()), true)?;
        }
        Ok(())
    }
}

fn register(exe: &Path, icon: &Path, dir: &Path) -> Result<()> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(KEY), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None).ok()?;
        let text = |name: &str, value: &str| {
            let wide: Vec<u16> = value.encode_utf16().chain([0]).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            let _ = RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(bytes));
        };
        text("DisplayName", "zunflix");
        text("DisplayVersion", crate::net::VERSION);
        text("Publisher", "zunflix");
        text("DisplayIcon", &icon.to_string_lossy());
        text("InstallLocation", &dir.to_string_lossy());
        text("UninstallString", &format!("\"{}\" --uninstall", exe.to_string_lossy()));
        for name in ["NoModify", "NoRepair"] {
            let one = 1u32.to_le_bytes();
            let _ = RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&one));
        }
        let _ = RegCloseKey(key);
        Ok(())
    }
}

/// Uninstalling from Settings: the uninstaller opens (already said yes to) and waits for zunflix to close.
pub fn uninstall_from_app() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(["--uninstall", "--confirmed", &format!("--wait={}", std::process::id())]).spawn();
    }
}
