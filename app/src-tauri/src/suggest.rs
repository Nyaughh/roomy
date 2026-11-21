//! Suggested cleanups. Only things that are genuinely safe to remove:
//! temporary files, caches that apps rebuild on their own, crash dumps and
//! the Recycle Bin. Anything involving the user's own content (project
//! build folders, duplicates) is opt-in and never pre-selected.

use crate::delete::Mode;
use crate::scan;
use crate::tree::{Tree, KIND_DIR};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// Safe for everyone.
    Recommended,
    /// Developer tool caches.
    Developer,
    /// The user's own files: review before removing.
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Delete everything inside each item folder (the folder stays).
    Contents,
    /// Delete each item path itself.
    Whole,
    /// Empty the Recycle Bin / Trash.
    RecycleBin,
    /// Nothing to delete directly: Roomy points to the system tool.
    Guide,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub path: String,
    pub label: String,
    pub size: u64,
    pub files: u64,
    pub mtime: i64,
    pub default_selected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: String,
    pub title: String,
    pub category: Category,
    pub what: String,
    pub why_safe: String,
    pub effect: String,
    pub note: Option<String>,
    pub action: Action,
    pub delete_mode: Mode,
    pub needs_admin: bool,
    pub size: u64,
    pub files: u64,
    pub items: Vec<Item>,
    /// Hours: files modified more recently than this are kept.
    pub min_age_hours: Option<u64>,
    pub settings_uri: Option<String>,
}

struct Spec {
    id: &'static str,
    title: &'static str,
    category: Category,
    what: &'static str,
    why_safe: &'static str,
    effect: &'static str,
    note: Option<&'static str>,
    action: Action,
    /// Path patterns. `{VAR}` placeholders, `*` matches one path segment.
    targets: &'static [&'static str],
    default_selected: bool,
    needs_admin: bool,
    min_age_hours: Option<u64>,
    /// General catch-all specs skip paths already claimed by specific ones.
    general: bool,
}

#[cfg(windows)]
const BROWSER_WHAT: &str =
    "Copies of web pages, images and scripts your browser saved so sites load a little faster.";
#[cfg(windows)]
const BROWSER_SAFE: &str = "Only the cache folders are touched. Bookmarks, passwords, history, cookies and open tabs are kept.";
#[cfg(windows)]
const BROWSER_EFFECT: &str = "Sites load slightly slower the first time you revisit them while the browser rebuilds its cache. Close the browser first for the biggest result; files it's using are skipped.";

#[cfg(windows)]
macro_rules! browser {
    ($id:literal, $title:literal, [$($t:literal),* $(,)?]) => {
        Spec {
            id: $id,
            title: $title,
            category: Category::Recommended,
            what: BROWSER_WHAT,
            why_safe: BROWSER_SAFE,
            effect: BROWSER_EFFECT,
            note: None,
            action: Action::Contents,
            targets: &[$($t),*],
            default_selected: true,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        }
    };
}

const DEV_SAFE: &str = "It's a download cache. The tool fetches anything it needs again automatically; your projects and installed tools are not affected.";

macro_rules! devcache {
    ($id:literal, $title:literal, $what:literal, $selected:expr, $note:expr, [$($t:literal),* $(,)?]) => {
        Spec {
            id: $id,
            title: $title,
            category: Category::Developer,
            what: $what,
            why_safe: DEV_SAFE,
            effect: "The next install or build may take longer and needs an internet connection to re-download packages.",
            note: $note,
            action: Action::Contents,
            targets: &[$($t),*],
            default_selected: $selected,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        }
    };
}

#[cfg(windows)]
fn specs() -> Vec<Spec> {
    vec![
        Spec {
            id: "user-temp",
            title: "Temporary files",
            category: Category::Recommended,
            what: "Leftovers that apps and installers put in your Temp folder and forgot to clean up.",
            why_safe: "Temp is meant to be disposable. Roomy only removes files untouched for at least a day, and skips anything in use.",
            effect: "Nothing you'll notice. Apps recreate what they need.",
            note: None,
            action: Action::Contents,
            targets: &["{TEMP}", "{LOCALAPPDATA}\\Temp"],
            default_selected: true,
            needs_admin: false,
            min_age_hours: Some(24),
            general: false,
        },
        Spec {
            id: "windows-temp",
            title: "Windows temporary files",
            category: Category::Recommended,
            what: "Temporary files left by Windows services and system installers.",
            why_safe: "Like your own Temp folder, this is scratch space. Files touched in the last day, or in use, are kept.",
            effect: "Nothing you'll notice.",
            note: None,
            action: Action::Contents,
            targets: &["{WINDIR}\\Temp"],
            default_selected: true,
            needs_admin: true,
            min_age_hours: Some(24),
            general: false,
        },
        Spec {
            id: "recycle-bin",
            title: "Recycle Bin",
            category: Category::Recommended,
            what: "Files you've already deleted. They still take up space until the bin is emptied.",
            why_safe: "You chose to delete these. Emptying the bin is the last step.",
            effect: "These files can no longer be restored.",
            note: Some("Have a quick look in the Recycle Bin first if you're not sure."),
            action: Action::RecycleBin,
            targets: &[],
            default_selected: false,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        Spec {
            id: "thumbnails",
            title: "Thumbnail & icon cache",
            category: Category::Recommended,
            what: "Small preview images File Explorer saves for photos, videos and icons.",
            why_safe: "Windows rebuilds them automatically. Files Explorer is currently using are skipped.",
            effect: "Folders full of photos may show previews a moment later the first time you open them.",
            note: None,
            action: Action::Whole,
            targets: &[
                "{LOCALAPPDATA}\\Microsoft\\Windows\\Explorer\\thumbcache_*.db",
                "{LOCALAPPDATA}\\Microsoft\\Windows\\Explorer\\iconcache_*.db",
            ],
            default_selected: true,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        Spec {
            id: "crash-dumps",
            title: "Crash dumps & error reports",
            category: Category::Recommended,
            what: "Memory snapshots and reports saved when an app or Windows crashed.",
            why_safe: "They're only useful for debugging a past crash, and any reports have already been sent or expired.",
            effect: "You can't send those old crash reports to a developer anymore.",
            note: None,
            action: Action::Contents,
            targets: &[
                "{LOCALAPPDATA}\\CrashDumps",
                "{LOCALAPPDATA}\\Microsoft\\Windows\\WER\\ReportArchive",
                "{LOCALAPPDATA}\\Microsoft\\Windows\\WER\\ReportQueue",
                "{PROGRAMDATA}\\Microsoft\\Windows\\WER\\ReportArchive",
                "{PROGRAMDATA}\\Microsoft\\Windows\\WER\\ReportQueue",
                "{WINDIR}\\Minidump",
                "{WINDIR}\\LiveKernelReports",
                "{WINDIR}\\MEMORY.DMP",
            ],
            default_selected: true,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        browser!("chrome-cache", "Chrome cache", [
            "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\Cache",
            "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\Code Cache",
            "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\*\\GPUCache",
            "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\ShaderCache",
            "{LOCALAPPDATA}\\Google\\Chrome\\User Data\\GrShaderCache",
        ]),
        browser!("edge-cache", "Microsoft Edge cache", [
            "{LOCALAPPDATA}\\Microsoft\\Edge\\User Data\\*\\Cache",
            "{LOCALAPPDATA}\\Microsoft\\Edge\\User Data\\*\\Code Cache",
            "{LOCALAPPDATA}\\Microsoft\\Edge\\User Data\\*\\GPUCache",
            "{LOCALAPPDATA}\\Microsoft\\Edge\\User Data\\ShaderCache",
            "{LOCALAPPDATA}\\Microsoft\\Edge\\User Data\\GrShaderCache",
        ]),
        browser!("brave-cache", "Brave cache", [
            "{LOCALAPPDATA}\\BraveSoftware\\Brave-Browser\\User Data\\*\\Cache",
            "{LOCALAPPDATA}\\BraveSoftware\\Brave-Browser\\User Data\\*\\Code Cache",
            "{LOCALAPPDATA}\\BraveSoftware\\Brave-Browser\\User Data\\*\\GPUCache",
        ]),
        browser!("vivaldi-cache", "Vivaldi cache", [
            "{LOCALAPPDATA}\\Vivaldi\\User Data\\*\\Cache",
            "{LOCALAPPDATA}\\Vivaldi\\User Data\\*\\Code Cache",
            "{LOCALAPPDATA}\\Vivaldi\\User Data\\*\\GPUCache",
        ]),
        browser!("opera-cache", "Opera cache", [
            "{LOCALAPPDATA}\\Opera Software\\Opera Stable\\Cache",
            "{LOCALAPPDATA}\\Opera Software\\Opera GX Stable\\Cache",
        ]),
        browser!("firefox-cache", "Firefox cache", [
            "{LOCALAPPDATA}\\Mozilla\\Firefox\\Profiles\\*\\cache2",
        ]),
        Spec {
            id: "app-caches",
            title: "App caches",
            category: Category::Recommended,
            what: "Caches kept by desktop apps built on web tech (Discord, VS Code, Slack, Teams, Steam's web views).",
            why_safe: "Only cache folders are touched. Your messages, settings, extensions and sign-ins stay as they are.",
            effect: "The app may start a little slower once while it rebuilds its cache. Files in use are skipped, so close the app first for the biggest result.",
            note: None,
            action: Action::Contents,
            targets: &[
                "{APPDATA}\\discord\\Cache",
                "{APPDATA}\\discord\\Code Cache",
                "{APPDATA}\\discord\\GPUCache",
                "{APPDATA}\\Code\\Cache",
                "{APPDATA}\\Code\\CachedData",
                "{APPDATA}\\Code\\CachedExtensionVSIXs",
                "{APPDATA}\\Code\\Code Cache",
                "{APPDATA}\\Code\\GPUCache",
                "{APPDATA}\\Slack\\Cache",
                "{APPDATA}\\Slack\\Code Cache",
                "{APPDATA}\\Slack\\GPUCache",
                "{APPDATA}\\Microsoft\\Teams\\Cache",
                "{APPDATA}\\Microsoft\\Teams\\Code Cache",
                "{APPDATA}\\Microsoft\\Teams\\GPUCache",
                "{LOCALAPPDATA}\\Steam\\htmlcache",
            ],
            default_selected: true,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        Spec {
            id: "shader-caches",
            title: "Graphics shader caches",
            category: Category::Recommended,
            what: "Compiled graphics shaders that DirectX and your GPU driver save for games and apps.",
            why_safe: "They're rebuilt automatically whenever they're needed.",
            effect: "Games may stutter a little or load slower the first time after cleaning while shaders are rebuilt.",
            note: None,
            action: Action::Contents,
            targets: &[
                "{LOCALAPPDATA}\\D3DSCache",
                "{LOCALAPPDATA}\\NVIDIA\\DXCache",
                "{LOCALAPPDATA}\\NVIDIA\\GLCache",
                "{LOCALAPPDATA}\\AMD\\DxCache",
                "{LOCALAPPDATA}\\AMD\\DxcCache",
                "{LOCALAPPDATA}\\AMD\\GLCache",
                "{LOCALAPPDATA}\\Intel\\ShaderCache",
            ],
            default_selected: false,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        Spec {
            id: "windows-update",
            title: "Windows Update downloads",
            category: Category::Recommended,
            what: "Installer files Windows downloaded for updates that are already installed, plus the Delivery Optimization cache used to share updates.",
            why_safe: "Windows downloads them again if it ever needs them. Files from the last week are kept so an update in progress isn't disturbed.",
            effect: "Nothing you'll notice. Installed updates stay installed.",
            note: None,
            action: Action::Contents,
            targets: &[
                "{WINDIR}\\SoftwareDistribution\\Download",
                "{WINDIR}\\ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache",
            ],
            default_selected: true,
            needs_admin: true,
            min_age_hours: Some(24 * 7),
            general: false,
        },
        Spec {
            id: "windows-old",
            title: "Previous Windows installation",
            category: Category::Review,
            what: "Windows.old holds your previous version of Windows after a major upgrade.",
            why_safe: "Windows' own cleanup tool removes it correctly. Roomy opens that tool for you instead of deleting system files itself.",
            effect: "You won't be able to roll back to the previous version of Windows.",
            note: Some("In Settings, choose Temporary files, then tick \u{201c}Previous Windows installation(s)\u{201d}."),
            action: Action::Guide,
            targets: &["{SYSTEMDRIVE}\\Windows.old"],
            default_selected: false,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        devcache!("npm-cache", "npm cache", "Packages npm downloaded before.", true, None, ["{LOCALAPPDATA}\\npm-cache"]),
        devcache!("pnpm-cache", "pnpm metadata cache", "Package metadata pnpm downloaded before.", true, None, ["{LOCALAPPDATA}\\pnpm-cache"]),
        devcache!("pnpm-store", "pnpm store", "pnpm's shared package store.", false,
            Some("Projects already installed keep working: their files are hard links. The space actually freed can be smaller than shown for the same reason."),
            ["{LOCALAPPDATA}\\pnpm\\store"]),
        devcache!("yarn-cache", "Yarn cache", "Packages Yarn downloaded before.", true, None,
            ["{LOCALAPPDATA}\\Yarn\\Cache", "{LOCALAPPDATA}\\Yarn\\Berry\\cache"]),
        devcache!("bun-cache", "Bun cache", "Packages Bun downloaded before.", false, None, ["{HOME}\\.bun\\install\\cache"]),
        devcache!("pip-cache", "pip cache", "Python packages pip downloaded before.", true, None, ["{LOCALAPPDATA}\\pip\\cache"]),
        devcache!("uv-cache", "uv cache", "Python packages uv downloaded before.", true, None, ["{LOCALAPPDATA}\\uv\\cache"]),
        devcache!("cargo-cache", "Cargo registry cache", "Rust crates Cargo downloaded and unpacked.", false, None,
            ["{HOME}\\.cargo\\registry\\cache", "{HOME}\\.cargo\\registry\\src", "{HOME}\\.cargo\\git\\checkouts"]),
        devcache!("gradle-cache", "Gradle caches", "Dependencies and build outputs Gradle cached.", false,
            Some("Stop running Gradle daemons first (gradle --stop)."),
            ["{HOME}\\.gradle\\caches"]),
        devcache!("nuget-cache", "NuGet HTTP cache", "Package downloads NuGet cached.", true, None,
            ["{LOCALAPPDATA}\\NuGet\\v3-cache", "{LOCALAPPDATA}\\NuGet\\plugins-cache"]),
        devcache!("go-build", "Go build cache", "Compiled Go packages cached to speed up builds.", true, None, ["{LOCALAPPDATA}\\go-build"]),
        devcache!("composer-cache", "Composer cache", "PHP packages Composer downloaded before.", true, None, ["{LOCALAPPDATA}\\Composer\\cache"]),
    ]
}

#[cfg(unix)]
fn specs() -> Vec<Spec> {
    let mut v = vec![
        Spec {
            id: "trash",
            title: "Trash",
            category: Category::Recommended,
            what: "Files you've already deleted. They still take up space until the Trash is emptied.",
            why_safe: "You chose to delete these. Emptying the Trash is the last step.",
            effect: "These files can no longer be restored.",
            note: Some("Have a quick look in the Trash first if you're not sure."),
            action: Action::Contents,
            targets: if cfg!(target_os = "macos") { &["{HOME}/.Trash"] } else { &["{HOME}/.local/share/Trash/files", "{HOME}/.local/share/Trash/info"] },
            default_selected: false,
            needs_admin: false,
            min_age_hours: None,
            general: false,
        },
        devcache!("npm-cache", "npm cache", "Packages npm downloaded before.", true, None, ["{HOME}/.npm/_cacache"]),
        devcache!("pnpm-store", "pnpm store", "pnpm's shared package store.", false,
            Some("Projects already installed keep working: their files are hard links. The space actually freed can be smaller than shown for the same reason."),
            ["{HOME}/Library/pnpm/store", "{HOME}/.local/share/pnpm/store"]),
        devcache!("bun-cache", "Bun cache", "Packages Bun downloaded before.", false, None, ["{HOME}/.bun/install/cache"]),
        devcache!("cargo-cache", "Cargo registry cache", "Rust crates Cargo downloaded and unpacked.", false, None,
            ["{HOME}/.cargo/registry/cache", "{HOME}/.cargo/registry/src", "{HOME}/.cargo/git/checkouts"]),
        devcache!("gradle-cache", "Gradle caches", "Dependencies and build outputs Gradle cached.", false,
            Some("Stop running Gradle daemons first (gradle --stop)."), ["{HOME}/.gradle/caches"]),
    ];
    if cfg!(target_os = "macos") {
        v.extend([
            Spec {
                id: "xcode-derived",
                title: "Xcode DerivedData",
                category: Category::Developer,
                what: "Intermediate build products and indexes Xcode keeps per project.",
                why_safe: "Xcode regenerates it on the next build. Source code isn't touched.",
                effect: "The next build of each project takes longer.",
                note: None,
                action: Action::Contents,
                targets: &["{HOME}/Library/Developer/Xcode/DerivedData"],
                default_selected: true,
                needs_admin: false,
                min_age_hours: None,
                general: false,
            },
            Spec {
                id: "ios-device-support",
                title: "iOS device support files",
                category: Category::Developer,
                what: "Debug symbols Xcode copied from every iPhone or iPad you've connected.",
                why_safe: "Xcode copies them again when you connect a device.",
                effect: "Connecting a device to debug takes a few extra minutes once.",
                note: None,
                action: Action::Contents,
                targets: &["{HOME}/Library/Developer/Xcode/iOS DeviceSupport"],
                default_selected: false,
                needs_admin: false,
                min_age_hours: None,
                general: false,
            },
            Spec {
                id: "crash-reports",
                title: "Crash reports & logs",
                category: Category::Recommended,
                what: "Diagnostic reports and logs saved by apps and macOS.",
                why_safe: "They're only useful for debugging past problems.",
                effect: "Old logs are no longer available for troubleshooting.",
                note: None,
                action: Action::Contents,
                targets: &["{HOME}/Library/Logs/DiagnosticReports", "{HOME}/Library/Logs/CoreSimulator"],
                default_selected: true,
                needs_admin: false,
                min_age_hours: None,
                general: false,
            },
            Spec {
                id: "user-caches",
                title: "App caches",
                category: Category::Recommended,
                what: "Caches apps keep in your Library to speed things up.",
                why_safe: "macOS treats this folder as disposable and apps rebuild what they need. Settings and documents live elsewhere.",
                effect: "Apps may be a little slower once while they rebuild caches. Quit apps first for the biggest result.",
                note: None,
                action: Action::Contents,
                targets: &["{HOME}/Library/Caches/*"],
                default_selected: false,
                needs_admin: false,
                min_age_hours: None,
                general: true,
            },
        ]);
    } else {
        v.push(Spec {
            id: "user-caches",
            title: "App caches",
            category: Category::Recommended,
            what: "Caches apps keep in ~/.cache, including thumbnails and package downloads.",
            why_safe: "This folder is meant to be disposable; apps rebuild what they need.",
            effect: "Apps may be a little slower once while they rebuild caches.",
            note: None,
            action: Action::Contents,
            targets: &["{HOME}/.cache/*"],
            default_selected: false,
            needs_admin: false,
            min_age_hours: None,
            general: true,
        });
    }
    v
}

fn env_var(name: &str) -> Option<String> {
    match name {
        "HOME" => dirs::home_dir().map(|p| p.to_string_lossy().into_owned()),
        "TEMP" => Some(std::env::temp_dir().to_string_lossy().into_owned()),
        "LOCALAPPDATA" => std::env::var("LOCALAPPDATA")
            .ok()
            .or_else(|| dirs::data_local_dir().map(|p| p.to_string_lossy().into_owned())),
        "APPDATA" => std::env::var("APPDATA")
            .ok()
            .or_else(|| dirs::config_dir().map(|p| p.to_string_lossy().into_owned())),
        "WINDIR" => std::env::var("SystemRoot").ok(),
        "SYSTEMDRIVE" => std::env::var("SystemDrive").ok(),
        "PROGRAMDATA" => std::env::var("ProgramData").ok(),
        _ => None,
    }
}

fn expand(pattern: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        out.push_str(&env_var(&rest[start + 1..end])?);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    let (p, n) = if cfg!(windows) {
        (pattern.to_lowercase(), name.to_lowercase())
    } else {
        (pattern.to_string(), name.to_string())
    };
    match p.split_once('*') {
        None => p == n,
        Some((pre, post)) => {
            n.len() >= pre.len() + post.len() && n.starts_with(pre) && n.ends_with(post)
        }
    }
}

/// Resolve a pattern with `*` segments into existing paths.
pub fn glob(pattern: &str) -> Vec<PathBuf> {
    let p = PathBuf::from(pattern);
    let mut current: Vec<PathBuf> = vec![PathBuf::new()];
    for comp in p.components() {
        let s = comp.as_os_str().to_string_lossy();
        if s.contains('*') {
            let mut next = Vec::new();
            for base in &current {
                if let Ok(rd) = std::fs::read_dir(base) {
                    for e in rd.flatten() {
                        if wildcard_match(&s, &e.file_name().to_string_lossy()) {
                            next.push(e.path());
                        }
                    }
                }
            }
            current = next;
        } else {
            for c in &mut current {
                c.push(comp.as_os_str());
            }
        }
    }
    current
        .into_iter()
        .filter(|c| {
            // Skip anything that is a link or sits below one: suggestions only
            // ever point at real folders.
            std::fs::symlink_metadata(c)
                .map(|m| !m.file_type().is_symlink())
                .unwrap_or(false)
                && crate::safety::linked_ancestor(c).is_none()
        })
        .collect()
}

fn norm(p: &Path) -> String {
    let s = p
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn label_for(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .rev()
        .take(3)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { "\\" } else { "/" })
}

/// Compute all path-based suggestions (independent of any scan).
pub fn compute(elevated: bool) -> Vec<Suggestion> {
    let cancel = Arc::new(AtomicBool::new(false));
    let all = specs();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut resolved: Vec<(usize, Vec<PathBuf>)> = Vec::new();
    // Specific specs first, so general ones can skip what's already claimed.
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by_key(|&i| all[i].general);
    for i in order {
        let spec = &all[i];
        let mut paths = Vec::new();
        for t in spec.targets {
            if let Some(exp) = expand(t) {
                for p in glob(&exp) {
                    let k = norm(&p);
                    if claimed.insert(k) {
                        paths.push(p);
                    }
                }
            }
        }
        resolved.push((i, paths));
    }
    resolved.sort_by_key(|(i, _)| *i);

    let mut out: Vec<Suggestion> = resolved
        .into_par_iter()
        .filter_map(|(i, paths)| {
            let spec = &all[i];
            let mut items: Vec<Item> = paths
                .par_iter()
                .map(|p| {
                    let (size, files) = scan::measure(p, cancel.clone());
                    let mtime = std::fs::symlink_metadata(p)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(scan::system_time_secs)
                        .unwrap_or(0);
                    Item {
                        path: p.to_string_lossy().into_owned(),
                        label: label_for(p),
                        size,
                        files,
                        mtime,
                        default_selected: spec.default_selected,
                    }
                })
                .filter(|it| it.size > 0)
                .collect();
            if spec.action == Action::RecycleBin {
                let (size, files) = crate::system::recycle_bin_size().unwrap_or((0, 0));
                if size > 0 {
                    items.push(Item {
                        path: "recycle-bin".into(),
                        label: "Recycle Bin (all drives)".into(),
                        size,
                        files,
                        mtime: 0,
                        default_selected: spec.default_selected,
                    });
                }
            }
            items.sort_by_key(|a| std::cmp::Reverse(a.size));
            let size: u64 = items.iter().map(|i| i.size).sum();
            if size < 1024 * 1024 {
                return None; // not worth showing
            }
            let needs_admin = spec.needs_admin && !elevated;
            Some(Suggestion {
                id: spec.id.into(),
                title: spec.title.into(),
                category: spec.category,
                what: spec.what.into(),
                why_safe: spec.why_safe.into(),
                effect: spec.effect.into(),
                note: spec.note.map(Into::into),
                action: spec.action,
                delete_mode: Mode::Permanent,
                needs_admin,
                size,
                files: items.iter().map(|i| i.files).sum(),
                items,
                min_age_hours: spec.min_age_hours,
                settings_uri: (spec.action == Action::Guide)
                    .then(|| "ms-settings:storagesense".into()),
            })
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.size));
    out
}

/// Folder names that are rebuildable build output, with the file that must
/// sit next to them to prove they belong to a project.
const PROJECT_JUNK: &[(&str, &[&str], &str)] = &[
    ("node_modules", &["package.json"], "Node.js packages"),
    ("target", &["Cargo.toml"], "Rust build output"),
    (".next", &["package.json"], "Next.js build output"),
    (".nuxt", &["package.json"], "Nuxt build output"),
    (".svelte-kit", &["package.json"], "SvelteKit build output"),
    (".turbo", &["package.json", "turbo.json"], "Turborepo cache"),
    (".parcel-cache", &["package.json"], "Parcel cache"),
    (".angular", &["angular.json"], "Angular cache"),
    (
        ".gradle",
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        "Gradle project cache",
    ),
    (
        ".venv",
        &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
        "Python virtual environment",
    ),
    (
        "venv",
        &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
        "Python virtual environment",
    ),
    (
        "obj",
        &["*.csproj", "*.fsproj", "*.vbproj"],
        ".NET build output",
    ),
    (
        "DerivedData",
        &["*.xcodeproj", "*.xcworkspace"],
        "Xcode build output",
    ),
];

/// Ancestors that mean "this isn't the user's project".
const NOT_PROJECTS: &[&str] = &[
    "appdata",
    "program files",
    "program files (x86)",
    "programdata",
    "windows",
    ".cargo",
    ".rustup",
    ".npm",
    ".nvm",
    ".vscode",
    ".cursor",
    "library",
    "node_modules",
    "$recycle.bin",
    ".git",
    "scoop",
    "system volume information",
    ".local",
    ".gradle",
    ".m2",
    "go",
];

fn name_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Find rebuildable project folders (node_modules, target, …) in a scan.
pub fn project_junk(tree: &Tree) -> Option<Suggestion> {
    const MIN: u64 = 10 * 1024 * 1024;
    let mut items = Vec::new();
    let mut stack = vec![0u32];
    while let Some(id) = stack.pop() {
        for c in tree.children(id) {
            let n = &tree.nodes[c as usize];
            if n.kind != KIND_DIR || n.size < MIN {
                continue;
            }
            let name = tree.name(c);
            let hit = PROJECT_JUNK.iter().find(|(dir, markers, _)| {
                name_eq(name, dir)
                    && tree.children(id).any(|s| {
                        let sn = tree.name(s);
                        markers.iter().any(|m| wildcard_match(m, sn))
                    })
            });
            if let Some((_, _, desc)) = hit {
                let project = tree.name(id);
                items.push(Item {
                    path: tree.path(c).to_string_lossy().into_owned(),
                    label: format!("{project} · {desc}"),
                    size: n.size,
                    files: n.files as u64,
                    mtime: tree.nodes[id as usize].mtime,
                    default_selected: false,
                });
                continue; // don't look inside
            }
            if !NOT_PROJECTS.iter().any(|x| name.eq_ignore_ascii_case(x)) {
                stack.push(c);
            }
        }
    }
    if items.is_empty() {
        return None;
    }
    items.sort_by_key(|a| std::cmp::Reverse(a.size));
    items.truncate(500);
    Some(Suggestion {
        id: "project-junk".into(),
        title: "Project build folders".into(),
        category: Category::Review,
        what: "Dependencies and build output inside your code projects: node_modules, Rust target folders, virtual environments and similar.".into(),
        why_safe: "Each one sits next to the project file that recreates it (package.json, Cargo.toml, …). Your source code isn't touched.".into(),
        effect: "You'll need to reinstall or rebuild a project (for example npm install or cargo build) before running it again.".into(),
        note: Some("Nothing here is selected for you. Pick the projects you aren't working on. The date shown is when the project folder last changed.".into()),
        action: Action::Whole,
        delete_mode: Mode::Permanent,
        needs_admin: false,
        size: items.iter().map(|i| i.size).sum(),
        files: items.iter().map(|i| i.files).sum(),
        items,
        min_age_hours: None,
        settings_uri: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{RawDir, RawEntry, KIND_FILE};

    #[test]
    fn wildcard() {
        assert!(wildcard_match("thumbcache_*.db", "thumbcache_256.db"));
        assert!(!wildcard_match("thumbcache_*.db", "thumbcache_256.dbx"));
        assert!(wildcard_match("*.csproj", "App.csproj"));
        assert!(wildcard_match(
            "Cache",
            if cfg!(windows) { "cache" } else { "Cache" }
        ));
    }

    #[test]
    fn glob_resolves_segments() {
        let tmp = tempfile::tempdir().unwrap();
        for p in [
            "User Data/Default/Cache",
            "User Data/Profile 1/Cache",
            "User Data/Default/Other",
        ] {
            std::fs::create_dir_all(tmp.path().join(p)).unwrap();
        }
        let pat = tmp.path().join("User Data").join("*").join("Cache");
        let mut found = glob(&pat.to_string_lossy());
        found.sort();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn glob_never_follows_links() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("Cache")).unwrap();
        let link = tmp.path().join("linked");
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .output()
            .unwrap()
            .status
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let through_link = glob(&link.join("Cache").to_string_lossy());
        assert!(through_link.is_empty(), "{through_link:?}");
        let star = glob(&tmp.path().join("*").join("Cache").to_string_lossy());
        assert_eq!(star, vec![real.join("Cache")]);
    }

    #[test]
    fn no_spec_targets_protected_paths() {
        // Every suggestion target must be deletable under the system guard.
        let guard = crate::safety::Guard::system();
        for s in specs() {
            for t in s.targets {
                let Some(exp) = expand(t) else { continue };
                let probe = PathBuf::from(exp.replace('*', "x"));
                let target = if s.action == Action::Contents {
                    probe.join("child")
                } else {
                    probe
                };
                if s.action == Action::Guide {
                    continue;
                }
                assert!(
                    guard.check(&target).is_ok(),
                    "{} target {} is blocked: {:?}",
                    s.id,
                    target.display(),
                    guard.check(&target)
                );
            }
        }
    }

    fn dir(name: &str, files: Vec<RawEntry>, dirs: Vec<RawDir>) -> RawDir {
        let mut d = RawDir::new(name.into(), 0);
        d.files = files;
        d.dirs = dirs;
        d.finish();
        d
    }

    fn f(name: &str, size: u64) -> RawEntry {
        RawEntry {
            name: name.into(),
            size,
            logical: size,
            mtime: 0,
            kind: KIND_FILE,
            flags: 0,
            key: 0,
        }
    }

    #[test]
    fn finds_project_junk_only_next_to_markers() {
        const MB: u64 = 1024 * 1024;
        let proj = dir(
            "webapp",
            vec![f("package.json", 1)],
            vec![dir("node_modules", vec![f("big", 50 * MB)], vec![])],
        );
        let stray = dir(
            "notes",
            vec![],
            vec![dir("node_modules", vec![f("big", 50 * MB)], vec![])],
        );
        let appdata = dir(
            "AppData",
            vec![],
            vec![dir(
                "tool",
                vec![f("package.json", 1)],
                vec![dir("node_modules", vec![f("x", 50 * MB)], vec![])],
            )],
        );
        let root = dir("root", vec![], vec![proj, stray, appdata]);
        let tree = Tree::from_raw(root, "/r".into());
        let s = project_junk(&tree).unwrap();
        assert_eq!(s.items.len(), 1);
        assert!(s.items[0].label.starts_with("webapp"));
        assert!(!s.items[0].default_selected);
    }
}
