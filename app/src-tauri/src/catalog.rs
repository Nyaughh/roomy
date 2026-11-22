//! What's installed: apps and games known to the OS and to game launchers.
//!
//! This is read once per scan from cheap sources (registry, launcher
//! manifests, app bundles). It never walks the disk: sizes come from the
//! scan tree when the storage view attributes paths to these records.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    App,
    Game,
    /// Windows/Store frameworks and built-in system apps.
    System,
}

/// How to remove an app the way its vendor intended. Roomy never deletes an
/// installed app's files itself.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Uninstall {
    /// The app's own uninstaller command line (from the registry).
    Command { command: String },
    /// A launcher URL, e.g. `steam://uninstall/570`.
    Url { url: String, label: String },
    /// Uninstall from a launcher Roomy can only open.
    Launcher { name: String, url: Option<String> },
    /// Windows Settings ▸ Apps.
    Settings,
    /// Show the app in Finder/Explorer so the user can remove it themselves
    /// (or run the vendor's own uninstaller). Roomy doesn't delete it.
    Reveal { path: String },
}

#[derive(Debug, Clone)]
pub struct AppRecord {
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub kind: AppKind,
    /// Where the program itself lives.
    pub install_dirs: Vec<PathBuf>,
    /// Known data folders (e.g. a Store app's package folder).
    pub data_dirs: Vec<PathBuf>,
    /// Known cache folders (e.g. Steam shader cache for a game).
    pub cache_dirs: Vec<PathBuf>,
    pub uninstall: Option<Uninstall>,
    pub source: &'static str,
    /// Extra names to match data folders by (exe stem, aliases, …).
    pub aliases: Vec<String>,
    /// Where the app's icon can be found, best first. Only recorded here;
    /// `icons` reads them lazily when the UI shows the app.
    pub icons: Vec<IconHint>,
}

/// A place an app's or game's icon may be found. Nothing is read while the
/// catalog loads: `icons` turns hints into image bytes on demand.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconHint {
    /// An icon resource (`.exe`, `.dll`, `.ico` with an index, as in the
    /// Windows `DisplayIcon` value) or an image file (`.png`, `.jpg`, `.icns`).
    File { path: PathBuf, index: i32 },
    /// The app's main executable somewhere inside this folder.
    ExeIn { dir: PathBuf },
    /// An MSIX/Appx package folder: the logo named in its AppxManifest.xml.
    Package { root: PathBuf },
    /// Steam's local library art (`appcache/librarycache`) for an app id.
    Steam { cache: PathBuf, appid: String },
    /// A macOS `.app` bundle.
    Bundle { app: PathBuf },
}

impl IconHint {
    /// Lower is better. Explicit icons beat launcher art, which beats
    /// guessing the main executable; uninstallers' icons come last.
    pub fn rank(&self) -> u8 {
        match self {
            IconHint::File { path, .. } => {
                // Both separators: registry paths are ranked the same everywhere.
                let p = path.to_string_lossy().to_lowercase();
                let file = p.rsplit(['/', '\\']).next().unwrap_or_default();
                if file.starts_with("unins") {
                    4
                } else {
                    0
                }
            }
            IconHint::Package { .. } | IconHint::Bundle { .. } => 0,
            IconHint::Steam { .. } => 1,
            IconHint::ExeIn { .. } => 2,
        }
    }
}

/// Order hints best first and drop duplicates.
pub fn tidy_icons(icons: &mut Vec<IconHint>) {
    let mut seen = std::collections::HashSet::new();
    icons.retain(|h| seen.insert(h.clone()));
    icons.sort_by_key(IconHint::rank);
}

/// Parse a Windows `DisplayIcon`-style value: `"C:\x\app.exe",0`,
/// `C:\x\app.ico`, `C:/x/icon.ico, -101`. Generic system hosts
/// (msiexec, rundll32, explorer) and folders are ignored.
pub fn icon_spec(s: &str) -> Option<IconHint> {
    let s = s.trim();
    let (path, rest) = if let Some(r) = s.strip_prefix('"') {
        let end = r.find('"')?;
        (&r[..end], r[end + 1..].trim())
    } else {
        match s.rfind(',') {
            Some(i) if s[i + 1..].trim().parse::<i32>().is_ok() => (&s[..i], &s[i..]),
            _ => (s, ""),
        }
    };
    let index = rest
        .trim_start_matches(',')
        .trim()
        .parse::<i32>()
        .unwrap_or(0);
    let path_str = path.trim();
    let drive = path_str.as_bytes().get(1) == Some(&b':');
    if !(drive || path_str.starts_with(['/', '\\'])) {
        return None;
    }
    let path = PathBuf::from(path_str);
    // Split on both separators so registry-style paths parse the same everywhere.
    let file = path_str.rsplit(['/', '\\']).next()?.to_lowercase();
    if file.is_empty() {
        return None;
    }
    if matches!(
        file.as_str(),
        "msiexec.exe" | "rundll32.exe" | "explorer.exe" | "cmd.exe" | "powershell.exe"
    ) {
        return None;
    }
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or_default();
    matches!(
        ext,
        "exe" | "dll" | "ico" | "icl" | "png" | "jpg" | "jpeg" | "icns"
    )
    .then_some(IconHint::File { path, index })
}

impl AppRecord {
    pub fn new(name: impl Into<String>, kind: AppKind, source: &'static str) -> Self {
        AppRecord {
            name: name.into(),
            publisher: None,
            version: None,
            kind,
            install_dirs: Vec::new(),
            data_dirs: Vec::new(),
            cache_dirs: Vec::new(),
            uninstall: None,
            source,
            aliases: Vec::new(),
            icons: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub apps: Vec<AppRecord>,
}

/// Lowercase letters and digits only: "Google Chrome (x64)" → "googlechromex64".
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// "Google LLC" → "google", "Valve Corporation" → "valve".
pub fn norm_publisher(s: &str) -> String {
    const SUFFIXES: &[&str] = &[
        "inc",
        "llc",
        "ltd",
        "limited",
        "corporation",
        "corp",
        "co",
        "gmbh",
        "ag",
        "sa",
        "srl",
        "ab",
        "bv",
        "plc",
        "pty",
        "software",
        "technologies",
        "technology",
        "studios",
        "games",
        "entertainment",
    ];
    let words: Vec<String> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    let mut end = words.len();
    while end > 1 && SUFFIXES.contains(&words[end - 1].as_str()) {
        end -= 1;
    }
    words[..end].concat()
}

/// Strip versions, architectures and parentheticals from a display name.
pub fn base_name(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let words: Vec<&str> = out
        .split_whitespace()
        .filter(|w| {
            let l = w.to_lowercase();
            let versionish = l
                .trim_start_matches('v')
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
                && l.chars().filter(|c| *c == '.').count() >= 1;
            !(versionish
                || matches!(
                    l.as_str(),
                    "x64" | "x86" | "64-bit" | "32-bit" | "-" | "–" | "amd64" | "arm64"
                ))
        })
        .collect();
    words.join(" ")
}

const GAME_PUBLISHERS: &[&str] = &[
    "riot",
    "blizzard",
    "electronicarts",
    "ea",
    "ubisoft",
    "rockstar",
    "bethesda",
    "bethesdasoftworks",
    "cdprojekt",
    "cdprojektred",
    "mojang",
    "activision",
    "bungie",
    "hoyoverse",
    "mihoyo",
    "cognosphere",
    "roblox",
    "gogcom",
    "gog",
    "squareenix",
    "bandainamco",
    "capcom",
    "sega",
    "2k",
    "valve",
    "wargaming",
    "gaijin",
    "nexon",
    "krafton",
];

pub fn is_game_publisher(p: &str) -> bool {
    let n = norm_publisher(p);
    GAME_PUBLISHERS.contains(&n.as_str())
}

/// Minimal Valve KeyValues reader: returns every ("key", "value") string pair
/// in document order (nesting is flattened, which is all Roomy needs).
pub fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let mut tokens: Vec<Option<String>> = Vec::new(); // None = brace
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                s.push(match n {
                                    'n' => '\n',
                                    't' => '\t',
                                    other => other,
                                });
                            }
                        }
                        '"' => break,
                        other => s.push(other),
                    }
                }
                tokens.push(Some(s));
            }
            '{' | '}' => tokens.push(None),
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        if let (Some(k), Some(v)) = (&tokens[i], &tokens[i + 1]) {
            out.push((k.clone(), v.clone()));
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Steam games from a Steam install folder (libraryfolders.vdf + appmanifests).
pub fn steam_games(steam_root: &Path) -> Vec<AppRecord> {
    let mut libraries: Vec<PathBuf> = vec![steam_root.to_path_buf()];
    let lf = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&lf) {
        for (k, v) in vdf_pairs(&text) {
            if k.eq_ignore_ascii_case("path") {
                let p = PathBuf::from(v);
                if !libraries
                    .iter()
                    .any(|l| norm(&l.to_string_lossy()) == norm(&p.to_string_lossy()))
                {
                    libraries.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    for lib in libraries {
        let steamapps = lib.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&steamapps) else {
            continue;
        };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let pairs = vdf_pairs(&text);
            let get = |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.clone())
            };
            let (Some(appid), Some(name), Some(dir)) =
                (get("appid"), get("name"), get("installdir"))
            else {
                continue;
            };
            // Steamworks redistributables aren't games. Ignore anything that
            // isn't a plain numeric app id or a single folder name.
            let plain_dir =
                !dir.is_empty() && dir != "." && dir != ".." && !dir.contains(['/', '\\', ':']);
            if appid == "228980"
                || appid.is_empty()
                || !appid.chars().all(|c| c.is_ascii_digit())
                || !plain_dir
            {
                continue;
            }
            let mut r = AppRecord::new(name, AppKind::Game, "Steam");
            r.publisher = Some("Steam".into());
            r.install_dirs.push(steamapps.join("common").join(&dir));
            r.data_dirs.push(steamapps.join("compatdata").join(&appid));
            r.data_dirs
                .push(steamapps.join("workshop").join("content").join(&appid));
            r.cache_dirs
                .push(steamapps.join("shadercache").join(&appid));
            r.uninstall = Some(Uninstall::Url {
                url: format!("steam://uninstall/{appid}"),
                label: "Steam's uninstall prompt".into(),
            });
            r.icons.push(IconHint::Steam {
                cache: steam_root.join("appcache").join("librarycache"),
                appid: appid.clone(),
            });
            r.icons.push(IconHint::ExeIn {
                dir: steamapps.join("common").join(&dir),
            });
            out.push(r);
        }
    }
    out
}

/// Epic Games Launcher installs (games and Unreal Engine versions).
pub fn epic_installs(program_data: &Path) -> Vec<AppRecord> {
    let mut out = Vec::new();
    let manifests = program_data
        .join(r"Epic")
        .join("EpicGamesLauncher")
        .join("Data")
        .join("Manifests");
    if let Ok(rd) = std::fs::read_dir(&manifests) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x != "item").unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let name = v["DisplayName"].as_str().unwrap_or_default();
            let loc = v["InstallLocation"].as_str().unwrap_or_default();
            if name.is_empty() || loc.is_empty() {
                continue;
            }
            let mut r = AppRecord::new(name, AppKind::Game, "Epic Games");
            r.install_dirs.push(PathBuf::from(loc));
            let exe = v["LaunchExecutable"].as_str().unwrap_or_default();
            if !exe.is_empty() && !exe.contains("..") {
                r.icons.push(IconHint::File {
                    path: Path::new(loc).join(exe),
                    index: 0,
                });
            }
            r.icons.push(IconHint::ExeIn {
                dir: PathBuf::from(loc),
            });
            r.uninstall = Some(Uninstall::Launcher {
                name: "Epic Games Launcher".into(),
                url: Some("com.epicgames.launcher://store/library".into()),
            });
            out.push(r);
        }
    }
    let installed = program_data
        .join("Epic")
        .join("UnrealEngineLauncher")
        .join("LauncherInstalled.dat");
    if let Ok(text) = std::fs::read_to_string(&installed) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            for it in v["InstallationList"].as_array().into_iter().flatten() {
                let app = it["AppName"].as_str().unwrap_or_default();
                let loc = it["InstallLocation"].as_str().unwrap_or_default();
                if loc.is_empty() {
                    continue;
                }
                let name = match app.strip_prefix("UE_") {
                    Some(ver) => format!("Unreal Engine {ver}"),
                    None => continue, // games are covered by the manifests above
                };
                let mut r = AppRecord::new(name, AppKind::App, "Epic Games");
                r.publisher = Some("Epic Games".into());
                r.install_dirs.push(PathBuf::from(loc));
                r.icons.push(IconHint::File {
                    path: Path::new(loc).join("Engine/Binaries/Win64/UnrealEditor.exe"),
                    index: 0,
                });
                r.uninstall = Some(Uninstall::Launcher {
                    name: "Epic Games Launcher".into(),
                    url: Some("com.epicgames.launcher://unreal".into()),
                });
                out.push(r);
            }
        }
    }
    out
}

/// Merge records that point at the same install folder (e.g. a Steam game
/// that also has a Windows uninstall entry). Store/launcher names win; an
/// uninstall command from the registry is kept.
pub fn merge(records: Vec<AppRecord>) -> Vec<AppRecord> {
    let mut out: Vec<AppRecord> = Vec::new();
    let mut by_dir: HashMap<String, usize> = HashMap::new();
    for r in records {
        let key = r.install_dirs.first().map(|d| norm(&d.to_string_lossy()));
        if let Some(k) = &key {
            if let Some(&i) = by_dir.get(k) {
                let existing = &mut out[i];
                let r_is_launcher = r.source != "Windows";
                if r_is_launcher && existing.source == "Windows" {
                    let keep_uninstall = existing.uninstall.clone();
                    let aliases = std::mem::take(&mut existing.aliases);
                    let icons = std::mem::take(&mut existing.icons);
                    *existing = r;
                    existing.aliases.extend(aliases);
                    existing.icons.extend(icons);
                    if let Some(Uninstall::Command { .. }) = keep_uninstall {
                        existing.uninstall = keep_uninstall;
                    }
                } else {
                    existing.data_dirs.extend(r.data_dirs);
                    existing.cache_dirs.extend(r.cache_dirs);
                    existing.aliases.extend(r.aliases);
                    existing.icons.extend(r.icons);
                    if existing.uninstall.is_none() {
                        existing.uninstall = r.uninstall;
                    }
                }
                continue;
            }
            by_dir.insert(k.clone(), out.len());
        }
        out.push(r);
    }
    for r in &mut out {
        tidy_icons(&mut r.icons);
    }
    out
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::scan::platform::to_wide;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::*;

    pub struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    pub fn open(root: HKEY, path: &str, wow64_32: bool) -> Option<Key> {
        let w = to_wide(path);
        let mut h: HKEY = std::ptr::null_mut();
        let view = if wow64_32 {
            KEY_WOW64_32KEY
        } else {
            KEY_WOW64_64KEY
        };
        let r = unsafe { RegOpenKeyExW(root, w.as_ptr(), 0, KEY_READ | view, &mut h) };
        (r == ERROR_SUCCESS).then_some(Key(h))
    }

    impl Key {
        pub fn subkeys(&self) -> Vec<String> {
            let mut out = Vec::new();
            let mut i = 0u32;
            loop {
                let mut buf = vec![0u16; 512];
                let mut len = buf.len() as u32;
                let r = unsafe {
                    RegEnumKeyExW(
                        self.0,
                        i,
                        buf.as_mut_ptr(),
                        &mut len,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if r != ERROR_SUCCESS {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
                i += 1;
            }
            out
        }

        pub fn open(&self, sub: &str) -> Option<Key> {
            let w = to_wide(sub);
            let mut h: HKEY = std::ptr::null_mut();
            let r = unsafe { RegOpenKeyExW(self.0, w.as_ptr(), 0, KEY_READ, &mut h) };
            (r == ERROR_SUCCESS).then_some(Key(h))
        }

        pub fn string(&self, name: &str) -> Option<String> {
            let w = to_wide(name);
            let mut ty = 0u32;
            let mut size = 0u32;
            let r = unsafe {
                RegQueryValueExW(
                    self.0,
                    w.as_ptr(),
                    std::ptr::null(),
                    &mut ty,
                    std::ptr::null_mut(),
                    &mut size,
                )
            };
            if r != ERROR_SUCCESS || !(ty == REG_SZ || ty == REG_EXPAND_SZ) || size == 0 {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            let r = unsafe {
                RegQueryValueExW(
                    self.0,
                    w.as_ptr(),
                    std::ptr::null(),
                    &mut ty,
                    buf.as_mut_ptr() as *mut u8,
                    &mut size,
                )
            };
            if r != ERROR_SUCCESS {
                return None;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let s = String::from_utf16_lossy(&buf[..len]).trim().to_string();
            if s.is_empty() {
                return None;
            }
            Some(if ty == REG_EXPAND_SZ {
                expand_env(&s)
            } else {
                s
            })
        }

        pub fn dword(&self, name: &str) -> Option<u32> {
            let w = to_wide(name);
            let mut ty = 0u32;
            let mut v = 0u32;
            let mut size = 4u32;
            let r = unsafe {
                RegQueryValueExW(
                    self.0,
                    w.as_ptr(),
                    std::ptr::null(),
                    &mut ty,
                    &mut v as *mut u32 as *mut u8,
                    &mut size,
                )
            };
            (r == ERROR_SUCCESS && ty == REG_DWORD).then_some(v)
        }
    }

    fn expand_env(s: &str) -> String {
        use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
        let w = to_wide(s);
        let mut buf = vec![0u16; 2048];
        let n =
            unsafe { ExpandEnvironmentStringsW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if n == 0 || n as usize > buf.len() {
            return s.to_string();
        }
        String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)])
    }

    fn load_indirect(s: &str) -> Option<String> {
        use windows_sys::Win32::UI::Shell::SHLoadIndirectString;
        let w = to_wide(s);
        let mut buf = vec![0u16; 512];
        let hr = unsafe {
            SHLoadIndirectString(
                w.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                std::ptr::null(),
            )
        };
        if hr < 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let out = String::from_utf16_lossy(&buf[..len]);
        (!out.is_empty() && !out.starts_with("ms-resource:")).then_some(out)
    }

    /// Path to an executable from a registry command/icon string.
    pub fn exe_from(s: &str) -> Option<PathBuf> {
        let s = s.trim();
        let path = if let Some(rest) = s.strip_prefix('"') {
            rest.split('"').next()?.to_string()
        } else {
            let lower = s.to_lowercase();
            match lower.find(".exe") {
                Some(i) => s[..i + 4].to_string(),
                None => s.split(',').next()?.to_string(),
            }
        };
        let p = PathBuf::from(path.trim());
        p.is_absolute().then_some(p)
    }

    fn usable_dir(p: &Path) -> bool {
        let s = p.to_string_lossy().to_lowercase();
        let windir = std::env::var("SystemRoot")
            .unwrap_or_else(|_| r"C:\Windows".into())
            .to_lowercase();
        !(s.starts_with(&windir)
            || s.contains(r"\package cache\")
            || s.contains(r"\installer\")
            || s.contains(r"\installshield installation information")
            || s.ends_with(r"\program files")
            || s.ends_with(r"\program files (x86)")
            || s.len() <= 3)
            && p.is_dir()
    }

    fn uninstall_entries(out: &mut Vec<AppRecord>) {
        const PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
        let roots = [
            (HKEY_LOCAL_MACHINE, false),
            (HKEY_LOCAL_MACHINE, true),
            (HKEY_CURRENT_USER, false),
        ];
        let mut seen = std::collections::HashSet::new();
        for (root, wow) in roots {
            let Some(k) = open(root, PATH, wow) else {
                continue;
            };
            for sub in k.subkeys() {
                let Some(e) = k.open(&sub) else { continue };
                let Some(name) = e.string("DisplayName") else {
                    continue;
                };
                if e.dword("SystemComponent") == Some(1) || e.string("ParentKeyName").is_some() {
                    continue;
                }
                if let Some(rt) = e.string("ReleaseType") {
                    if rt.contains("Update") || rt.contains("Hotfix") {
                        continue;
                    }
                }
                if !seen.insert((norm(&name), sub.to_lowercase())) {
                    continue;
                }
                let publisher = e.string("Publisher");
                let mut install: Option<PathBuf> = e
                    .string("InstallLocation")
                    .map(|s| PathBuf::from(s.trim_matches('"')));
                let display_icon = e.string("DisplayIcon");
                let icon_exe = display_icon.as_deref().and_then(exe_from);
                if !install.as_deref().map(usable_dir).unwrap_or(false) {
                    install = icon_exe
                        .as_ref()
                        .and_then(|p| p.parent().map(Path::to_path_buf))
                        .filter(|p| usable_dir(p));
                }
                let uninstall_cmd = e.string("UninstallString");
                if install.is_none() {
                    if let Some(cmd) = &uninstall_cmd {
                        if !cmd.to_lowercase().contains("msiexec") {
                            install = exe_from(cmd)
                                .and_then(|p| p.parent().map(Path::to_path_buf))
                                .filter(|p| usable_dir(p));
                        }
                    }
                }
                let game_by_path = install
                    .as_ref()
                    .map(|p| {
                        let s = p.to_string_lossy().to_lowercase();
                        s.contains(r"\steamapps\common\")
                            || s.contains(r"\games\")
                            || s.contains(r"\xboxgames\")
                    })
                    .unwrap_or(false);
                let is_game = (publisher.as_deref().map(is_game_publisher).unwrap_or(false)
                    || game_by_path)
                    && !name.to_lowercase().contains("launcher")
                    && !name.to_lowercase().contains("galaxy")
                    && !name.to_lowercase().contains("battle.net")
                    && !name.eq_ignore_ascii_case("steam");
                let mut r = AppRecord::new(
                    name,
                    if is_game { AppKind::Game } else { AppKind::App },
                    "Windows",
                );
                r.publisher = publisher;
                r.version = e.string("DisplayVersion");
                if let Some(h) = display_icon.as_deref().and_then(icon_spec) {
                    r.icons.push(h);
                }
                if let Some(d) = install {
                    r.icons.push(IconHint::ExeIn { dir: d.clone() });
                    r.install_dirs.push(d);
                }
                if let Some(exe) = &icon_exe {
                    if let Some(stem) = exe.file_stem() {
                        r.aliases.push(stem.to_string_lossy().into_owned());
                    }
                }
                r.uninstall = uninstall_cmd.map(|command| Uninstall::Command { command });
                out.push(r);
            }
        }
    }

    const FRAMEWORK_PREFIXES: &[&str] = &[
        "microsoft.vclibs",
        "microsoft.net.",
        "microsoft.ui.xaml",
        "microsoft.windowsappruntime",
        "microsoft.services.store",
        "microsoft.directxruntime",
        "microsoftwindows.",
        "windows.",
        "microsoft.windows.",
        "microsoft.languageexperiencepack",
        "microsoft.advertising",
        "microsoft.desktopappinstaller",
        "microsoft.storepurchaseapp",
        "microsoft.winappruntime",
        "microsoft.webpimageextension",
        "microsoft.heifimageextension",
        "microsoft.vp9videoextensions",
        "microsoft.webmediaextensions",
        "microsoft.hevcvideoextension",
        "microsoft.rawimageextension",
        "microsoft.av1videoextension",
        "microsoft.mpeg2videoextension",
        "microsoft.sechealthui",
        "microsoft.aad.",
        "microsoft.accountscontrol",
        "microsoft.lockapp",
        "microsoft.creddialoghost",
        "microsoft.ecapp",
        "microsoft.bioenrollment",
        "ncsiuwpapp",
        "microsoft.xboxgamecallableui",
        "microsoft.asynctextservice",
        "microsoft.win32webviewhost",
        "1527c705-839a",
        "c5e2524a-ea46",
        "e2a4f912-2574",
        "f46d4000-fd22",
        "microsoftcorporationii.",
        "clipchamp.",
        "aad.",
    ];

    /// "SpotifyAB.SpotifyMusic" → "Spotify Music".
    fn pretty_package(pkg: &str) -> String {
        let tail = pkg.rsplit('.').next().unwrap_or(pkg);
        let mut out = String::new();
        for (i, ch) in tail.chars().enumerate() {
            if i > 0 && ch.is_uppercase() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(ch);
        }
        out
    }

    fn store_packages(out: &mut Vec<AppRecord>) {
        const PATH: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
        let Some(k) = open(HKEY_CURRENT_USER, PATH, false) else {
            return;
        };
        let local = std::env::var("LOCALAPPDATA").map(PathBuf::from).ok();
        let mut by_family: HashMap<String, usize> = HashMap::new();
        for full in k.subkeys() {
            let parts: Vec<&str> = full.split('_').collect();
            if parts.len() < 5 {
                continue;
            }
            let pkg = parts[0];
            let family = format!("{}_{}", pkg, parts[parts.len() - 1]);
            let Some(e) = k.open(&full) else { continue };
            let root = e.string("PackageRootFolder").map(PathBuf::from);
            let lower = pkg.to_lowercase();
            let windir = std::env::var("SystemRoot")
                .unwrap_or_default()
                .to_lowercase();
            let is_system = FRAMEWORK_PREFIXES.iter().any(|p| lower.starts_with(p))
                || root
                    .as_ref()
                    .map(|r| r.to_string_lossy().to_lowercase().starts_with(&windir))
                    .unwrap_or(false);
            let display = e
                .string("DisplayName")
                .and_then(|d| {
                    if d.starts_with('@') {
                        load_indirect(&d)
                    } else if let Some(key) = d.strip_prefix("ms-resource:") {
                        let key = key.trim_start_matches('/');
                        let key = if key.contains('/') {
                            key.to_string()
                        } else {
                            format!("Resources/{key}")
                        };
                        load_indirect(&format!("@{{{full}?ms-resource://{pkg}/{key}}}"))
                    } else {
                        Some(d)
                    }
                })
                .unwrap_or_else(|| pretty_package(pkg));
            if let Some(&i) = by_family.get(&family) {
                if let Some(r) = root {
                    // Resource ("split") packages hold no app logo: the
                    // main package's logo goes first.
                    let hint = IconHint::Package { root: r.clone() };
                    if parts[3].is_empty() {
                        out[i].icons.insert(0, hint);
                    } else {
                        out[i].icons.push(hint);
                    }
                    out[i].install_dirs.push(r);
                }
                continue;
            }
            let in_xbox = root
                .as_ref()
                .map(|r| r.to_string_lossy().to_lowercase().contains("xboxgames"))
                .unwrap_or(false);
            let kind = if is_system {
                AppKind::System
            } else if in_xbox {
                AppKind::Game
            } else {
                AppKind::App
            };
            let mut r = AppRecord::new(display, kind, "Microsoft Store");
            if let Some(root) = root {
                r.icons.push(IconHint::Package { root: root.clone() });
                r.install_dirs.push(root);
            }
            if let Some(l) = &local {
                r.data_dirs.push(l.join("Packages").join(&family));
            }
            r.aliases
                .push(pkg.rsplit('.').next().unwrap_or(pkg).to_string());
            r.uninstall = Some(Uninstall::Settings);
            by_family.insert(family, out.len());
            out.push(r);
        }
    }

    fn gog_and_ubisoft(out: &mut Vec<AppRecord>) {
        if let Some(k) = open(HKEY_LOCAL_MACHINE, r"Software\GOG.com\Games", true) {
            for sub in k.subkeys() {
                let Some(e) = k.open(&sub) else { continue };
                let (Some(name), Some(path)) = (e.string("gameName"), e.string("path")) else {
                    continue;
                };
                let mut r = AppRecord::new(name, AppKind::Game, "GOG");
                if let Some(h) = e.string("exe").as_deref().and_then(icon_spec) {
                    r.icons.push(h);
                }
                r.icons.push(IconHint::ExeIn {
                    dir: PathBuf::from(&path),
                });
                r.install_dirs.push(PathBuf::from(path));
                r.uninstall = Some(Uninstall::Launcher {
                    name: "GOG Galaxy".into(),
                    url: None,
                });
                out.push(r);
            }
        }
        if let Some(k) = open(
            HKEY_LOCAL_MACHINE,
            r"Software\Ubisoft\Launcher\Installs",
            true,
        ) {
            for sub in k.subkeys() {
                let Some(e) = k.open(&sub) else { continue };
                let Some(dir) = e.string("InstallDir") else {
                    continue;
                };
                let p = PathBuf::from(dir.trim_end_matches(['/', '\\']));
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let mut r = AppRecord::new(name, AppKind::Game, "Ubisoft Connect");
                r.icons.push(IconHint::ExeIn { dir: p.clone() });
                r.install_dirs.push(p);
                r.uninstall = Some(Uninstall::Launcher {
                    name: "Ubisoft Connect".into(),
                    url: Some("uplay://".into()),
                });
                out.push(r);
            }
        }
    }

    pub fn load() -> Catalog {
        let mut records = Vec::new();
        // Launcher records first so their names win when merged.
        if let Some(k) = open(HKEY_CURRENT_USER, r"Software\Valve\Steam", false) {
            if let Some(p) = k.string("SteamPath") {
                records.extend(steam_games(&PathBuf::from(p.replace('/', "\\"))));
            }
        }
        if let Ok(pd) = std::env::var("ProgramData") {
            records.extend(epic_installs(Path::new(&pd)));
        }
        gog_and_ubisoft(&mut records);
        uninstall_entries(&mut records);
        store_packages(&mut records);
        Catalog {
            apps: merge(records),
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    pub fn load() -> Catalog {
        let mut out = Vec::new();
        let mut dirs = vec![PathBuf::from("/Applications")];
        if let Some(h) = dirs::home_dir() {
            dirs.push(h.join("Applications"));
        }
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x != "app").unwrap_or(true) {
                    continue;
                }
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut r = AppRecord::new(name, AppKind::App, "Applications");
                r.icons.push(IconHint::Bundle { app: p.clone() });
                r.install_dirs.push(p.clone());
                r.uninstall = Some(Uninstall::Reveal {
                    path: p.to_string_lossy().into_owned(),
                });
                out.push(r);
            }
        }
        if let Some(h) = dirs::home_dir() {
            let steam = h.join("Library/Application Support/Steam");
            out.extend(steam_games(&steam));
        }
        Catalog { apps: merge(out) }
    }
}

pub fn load() -> Catalog {
    #[cfg(windows)]
    {
        win::load()
    }
    #[cfg(target_os = "macos")]
    {
        mac::load()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut c = Catalog::default();
        if let Some(h) = dirs::home_dir() {
            c.apps.extend(steam_games(&h.join(".local/share/Steam")));
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(norm("Google Chrome (x64)"), "googlechromex64");
        assert_eq!(norm_publisher("Google LLC"), "google");
        assert_eq!(norm_publisher("Valve Corporation"), "valve");
        assert_eq!(norm_publisher("Riot Games, Inc"), "riot");
        assert_eq!(base_name("7-Zip 23.01 (x64)"), "7-Zip");
        assert_eq!(base_name("Python 3.12.1 (64-bit)"), "Python");
        assert!(is_game_publisher("Riot Games, Inc"));
        assert!(!is_game_publisher("Google LLC"));
    }

    #[test]
    fn parses_steam_library() {
        let tmp = tempfile::tempdir().unwrap();
        let steam = tmp.path().join("Steam");
        let lib2 = tmp.path().join("Lib2");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib2.join("steamapps")).unwrap();
        let lib2_escaped = lib2.to_string_lossy().replace('\\', "\\\\");
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{lib2_escaped}\"\n\t}}\n}}\n"),
        )
        .unwrap();
        std::fs::write(
            lib2.join("steamapps/appmanifest_570.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"installdir\"\t\t\"dota 2 beta\"\n}\n",
        )
        .unwrap();
        // Manifests with a non-numeric id or a path-like install dir are ignored.
        std::fs::write(
            lib2.join("steamapps/appmanifest_1.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"../../x\"\n\t\"name\"\t\t\"Bad\"\n\t\"installdir\"\t\t\"y\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            lib2.join("steamapps/appmanifest_2.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"2\"\n\t\"name\"\t\t\"Bad2\"\n\t\"installdir\"\t\t\"..\"\n}\n",
        )
        .unwrap();
        let games = steam_games(&steam);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Dota 2");
        assert_eq!(games[0].kind, AppKind::Game);
        assert!(games[0].install_dirs[0].ends_with("dota 2 beta"));
        assert!(
            matches!(&games[0].uninstall, Some(Uninstall::Url { url, .. }) if url == "steam://uninstall/570")
        );
    }

    #[test]
    fn parses_display_icon_values() {
        let f = |s: &str| match icon_spec(s) {
            Some(IconHint::File { path, index }) => Some((path, index)),
            _ => None,
        };
        assert_eq!(
            f(r#""C:\Program Files\App\app.exe",0"#),
            Some((PathBuf::from(r"C:\Program Files\App\app.exe"), 0))
        );
        assert_eq!(
            f(r"C:\x\OneDriveSetup.exe,-101"),
            Some((PathBuf::from(r"C:\x\OneDriveSetup.exe"), -101))
        );
        assert_eq!(
            f(r"C:\Rockstar\Launcher.exe, 0"),
            Some((PathBuf::from(r"C:\Rockstar\Launcher.exe"), 0))
        );
        assert_eq!(
            f("C:/Riot Games/Riot Client/Resources/icon.ico"),
            Some((
                PathBuf::from("C:/Riot Games/Riot Client/Resources/icon.ico"),
                0
            ))
        );
        assert_eq!(f(r"C:\Windows\System32\msiexec.exe,1"), None);
        assert_eq!(f(r"C:\Program Files\qpdf 12.3.2\"), None);
        assert_eq!(f("app.exe"), None);
        assert_eq!(f(""), None);
    }

    #[test]
    fn merged_records_keep_the_best_icon_first() {
        let dir = PathBuf::from(r"D:\Steam\steamapps\common\Game");
        let mut steam = AppRecord::new("Game", AppKind::Game, "Steam");
        steam.install_dirs.push(dir.clone());
        steam.icons.push(IconHint::Steam {
            cache: PathBuf::from(r"D:\Steam\appcache\librarycache"),
            appid: "1".into(),
        });
        steam.icons.push(IconHint::ExeIn { dir: dir.clone() });
        let mut reg = AppRecord::new("Game", AppKind::Game, "Windows");
        reg.install_dirs.push(dir.clone());
        let ico = IconHint::File {
            path: PathBuf::from(r"D:\Steam\steam\games\abc.ico"),
            index: 0,
        };
        reg.icons.push(ico.clone());
        reg.icons.push(IconHint::ExeIn { dir: dir.clone() });
        let m = merge(vec![reg, steam]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].icons.len(), 3, "duplicates dropped");
        assert_eq!(m[0].icons[0], ico);
        assert!(matches!(m[0].icons[1], IconHint::Steam { .. }));
        assert!(matches!(m[0].icons[2], IconHint::ExeIn { .. }));
        // An uninstaller's icon is only a last resort.
        let mut v = vec![
            IconHint::File {
                path: PathBuf::from(r"C:\A\unins000.exe"),
                index: 0,
            },
            IconHint::ExeIn {
                dir: PathBuf::from(r"C:\A"),
            },
        ];
        tidy_icons(&mut v);
        assert!(matches!(v[0], IconHint::ExeIn { .. }));
    }

    #[test]
    fn merges_by_install_dir() {
        let mut steam = AppRecord::new("Dota 2", AppKind::Game, "Steam");
        steam
            .install_dirs
            .push(PathBuf::from(r"D:\Steam\steamapps\common\dota 2 beta"));
        let mut reg = AppRecord::new("Steam App 570", AppKind::App, "Windows");
        reg.install_dirs
            .push(PathBuf::from(r"D:\Steam\steamapps\common\dota 2 beta"));
        reg.uninstall = Some(Uninstall::Command {
            command: "steam.exe steam://uninstall/570".into(),
        });
        let m = merge(vec![steam, reg]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "Dota 2");
        assert!(matches!(m[0].uninstall, Some(Uninstall::Command { .. })));
    }
}
