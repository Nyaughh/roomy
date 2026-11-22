//! Real icons for apps and games in the Storage view.
//!
//! Nothing here runs during a scan. The catalog only records *hints* (a
//! `DisplayIcon` value, an install folder, a package folder, a Steam app id,
//! a `.app` bundle). When the UI shows a row it asks for that entity's icon
//! key; this module resolves the hints on a small low-priority pool, reads or
//! extracts the image and caches it in memory and on disk (keyed by the
//! source file's path, size and modification time, so updated apps get their
//! new icon). Package logos and Steam art are small images used as they are;
//! `.exe`/`.dll`/`.ico` resources (Windows) and `.icns` files (macOS) are
//! extracted once and stored as PNG.

use crate::catalog::{norm, IconHint};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Pixel size icons are extracted at (tiles are 30–44 CSS px, so this stays
/// sharp on high-DPI screens).
pub const ICON_PX: u32 = 64;
/// Images larger than this are not icons; skip them.
const MAX_IMAGE_BYTES: u64 = 512 * 1024;
/// Bump to invalidate every cached extraction.
const CACHE_VERSION: &str = "1";
/// The on-disk cache is cleared when it grows past this many files.
const MAX_CACHE_FILES: usize = 4000;
/// At most this many keys are served per request.
pub const MAX_KEYS_PER_CALL: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl Icon {
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, base64(&self.bytes))
    }
}

/// A concrete file an icon can be read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A PNG or JPEG used as it is.
    Image { path: PathBuf, mime: &'static str },
    /// An icon resource that has to be extracted (`.exe`, `.dll`, `.ico`, `.icns`).
    Resource { path: PathBuf, index: i32 },
}

/// Stable short key for a list of hints (the UI caches icons by it).
pub fn key(hints: &[IconHint]) -> Option<String> {
    if hints.is_empty() {
        return None;
    }
    let mut h = blake3::Hasher::new();
    for x in hints {
        h.update(format!("{x:?}\n").as_bytes());
    }
    Some(h.finalize().to_hex()[..16].to_string())
}

fn ext_lower(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn read_small(p: &Path, max: u64) -> Option<Vec<u8>> {
    let md = std::fs::metadata(p).ok()?;
    if !md.is_file() || md.len() == 0 || md.len() > max {
        return None;
    }
    std::fs::read(p).ok()
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// Concrete files a hint points at, best first. Cheap: a few stats, one
/// manifest read or a shallow folder listing.
pub fn resolve(hint: &IconHint) -> Vec<Source> {
    match hint {
        IconHint::File { path, index } => file_source(path, *index).into_iter().collect(),
        IconHint::ExeIn { dir } => main_exes(dir)
            .into_iter()
            .map(|path| Source::Resource { path, index: 0 })
            .collect(),
        IconHint::Package { root } => package_logo(root)
            .map(|path| Source::Image {
                path,
                mime: "image/png",
            })
            .into_iter()
            .collect(),
        IconHint::Steam { cache, appid } => steam_icon(cache, appid).into_iter().collect(),
        IconHint::Bundle { app } => bundle_icns(app)
            .map(|path| Source::Resource { path, index: 0 })
            .into_iter()
            .collect(),
    }
}

fn file_source(path: &Path, index: i32) -> Option<Source> {
    if !path.is_file() {
        return None;
    }
    let path = path.to_path_buf();
    Some(match ext_lower(&path).as_str() {
        "png" => Source::Image {
            path,
            mime: "image/png",
        },
        "jpg" | "jpeg" => Source::Image {
            path,
            mime: "image/jpeg",
        },
        _ => Source::Resource { path, index },
    })
}

// --- Main executable of an install folder ---------------------------------

#[derive(Debug, Clone)]
pub struct ExeCandidate {
    pub stem: String,
    pub depth: u8,
    pub size: u64,
}

/// Executables that are never the app itself.
const NOT_MAIN_EXE: &[&str] = &[
    "unins",
    "uninstall",
    "crash",
    "report",
    "redist",
    "dxsetup",
    "prereq",
    "helper",
    "elevat",
    "notification",
    "squirrel",
    "createdump",
    "installer",
    "setup",
    "cefsharp",
    "easyanticheat",
    "battleye",
];

/// Folders that never hold the main executable.
const SKIP_DIRS: &[&str] = &[
    "locales",
    "redist",
    "_commonredist",
    "commonredist",
    "uninstall",
    "installer",
    "crashpad",
    "swiftshader",
    "node_modules",
    ".git",
    "easyanticheat",
    "battleye",
    "__installer",
];

/// Pick the executable that most likely is the app: its name matches the
/// folder, it sits near the top, and it's big.
pub fn pick_main_exe(dir_name: &str, candidates: &[ExeCandidate]) -> Option<usize> {
    rank_exes(dir_name, candidates).first().copied()
}

/// Candidate indices, most likely main executable first (helpers dropped).
pub fn rank_exes(dir_name: &str, candidates: &[ExeCandidate]) -> Vec<usize> {
    let want = norm(dir_name);
    let mut ranked: Vec<(i64, u64, usize)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let l = c.stem.to_lowercase();
            !NOT_MAIN_EXE.iter().any(|n| l.contains(n))
        })
        .map(|(i, c)| {
            let s = norm(&c.stem);
            let mut score: i64 = 0;
            if !want.is_empty() && s == want {
                score += 100;
            } else if s.len() >= 3 && want.len() >= 3 && (want.contains(&s) || s.contains(&want)) {
                score += 60;
            }
            if s.contains("update") || s.contains("launch") {
                score -= 20;
            }
            score -= 15 * c.depth as i64;
            (score, c.size, i)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    ranked.into_iter().map(|(_, _, i)| i).collect()
}

/// Folders worth following below the second level, where launchers and
/// engines keep their binaries (e.g. `Launcher\Portal\Binaries\Win64`).
const BIN_DIRS: &[&str] = &[
    "bin",
    "binaries",
    "win64",
    "x64",
    "portal",
    "launcher",
    "app",
    "application",
    "program",
];

/// Executables in `dir` (top level plus two levels down, bounded).
fn exe_candidates(dir: &Path) -> Vec<(PathBuf, ExeCandidate)> {
    const MAX_DIRS: usize = 48;
    const MAX_ENTRIES: usize = 400;
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([(dir.to_path_buf(), 0u8)]);
    let mut visited = 0;
    while let Some((d, depth)) = queue.pop_front() {
        visited += 1;
        if visited > MAX_DIRS {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten().take(MAX_ENTRIES) {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                let follow = depth < 2 || (depth < 4 && BIN_DIRS.contains(&name.as_str()));
                if follow && !SKIP_DIRS.contains(&name.as_str()) {
                    queue.push_back((p, depth + 1));
                }
            } else if ft.is_file() && ext_lower(&p) == "exe" {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push((p, ExeCandidate { stem, depth, size }));
            }
        }
    }
    out
}

/// The few most likely main executables. Several, because some candidates
/// (command-line tools, plug-ins) carry no icon and the next one does.
fn main_exes(dir: &Path) -> Vec<PathBuf> {
    let found = exe_candidates(dir);
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cands: Vec<ExeCandidate> = found.iter().map(|(_, c)| c.clone()).collect();
    rank_exes(&name, &cands)
        .into_iter()
        .take(4)
        .map(|i| found[i].0.clone())
        .collect()
}

// --- MSIX / Appx package logos ---------------------------------------------

fn xml_attr(text: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let i = text.find(&pat)? + pat.len();
    let end = text[i..].find('"')?;
    Some(text[i..i + end].to_string())
}

fn xml_elem(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let i = text.find(&open)? + open.len();
    let end = text[i..].find('<')?;
    Some(text[i..i + end].trim().to_string())
}

/// Choose the best file for an MSIX logo among its qualified variants:
/// `Logo.targetsize-64_altform-unplated.png` (transparent, right size) beats
/// `Logo.scale-200.png` (plated) beats `Logo.png`. High-contrast variants are
/// never used.
pub fn pick_logo_variant(stem: &str, ext: &str, names: &[String]) -> Option<String> {
    let stem_l = stem.to_lowercase();
    let ext_l = format!(".{}", ext.to_lowercase());
    let mut best: Option<(i64, &String)> = None;
    for n in names {
        let l = n.to_lowercase();
        let Some(rest) = l.strip_prefix(&stem_l) else {
            continue;
        };
        let Some(quals) = rest.strip_suffix(&ext_l) else {
            continue;
        };
        let score = if quals.is_empty() {
            40
        } else if let Some(q) = quals.strip_prefix('.') {
            if q.contains("contrast-") {
                continue;
            }
            let mut score = 0i64;
            for part in q.split('_') {
                if let Some(n) = part.strip_prefix("targetsize-") {
                    let n: i64 = n.parse().unwrap_or(0);
                    score += 300 - (n - ICON_PX as i64).abs();
                    if n < 48 {
                        score -= 150;
                    }
                } else if let Some(s) = part.strip_prefix("scale-") {
                    let s: i64 = s.parse().unwrap_or(100);
                    score += 100 - (s - 200).abs() / 10;
                } else if part == "altform-unplated" {
                    score += 60;
                } else if part == "altform-lightunplated" {
                    score += 50;
                }
            }
            score
        } else {
            continue;
        };
        if best.map(|(b, _)| score > b).unwrap_or(true) {
            best = Some((score, n));
        }
    }
    best.map(|(_, n)| n.clone())
}

fn package_logo(root: &Path) -> Option<PathBuf> {
    let bytes = read_small(&root.join("AppxManifest.xml"), 1024 * 1024)?;
    let text = String::from_utf8_lossy(&bytes);
    let rel = xml_attr(&text, "Square44x44Logo").or_else(|| xml_elem(&text, "Logo"))?;
    let rel = rel.replace('\\', "/");
    if rel.is_empty() || rel.contains("..") || rel.starts_with('/') || rel.contains(':') {
        return None;
    }
    let full = root.join(&rel);
    let dir = full.parent()?;
    let stem = full.file_stem()?.to_string_lossy().into_owned();
    let ext = ext_lower(&full);
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .take(2000)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    pick_logo_variant(&stem, &ext, &names).map(|n| dir.join(n))
}

// --- Steam library art -------------------------------------------------------

fn is_sha1_jpg(name: &str) -> bool {
    let l = name.to_lowercase();
    l.strip_suffix(".jpg")
        .map(|s| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false)
}

/// Steam keeps each game's small icon in `librarycache/<appid>/<sha1>.jpg`
/// (older clients: `librarycache/<appid>_icon.jpg`).
fn steam_icon(cache: &Path, appid: &str) -> Option<Source> {
    if appid.is_empty() || !appid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Ok(rd) = std::fs::read_dir(cache.join(appid)) {
        let mut icons: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| is_sha1_jpg(&e.file_name().to_string_lossy()))
            .map(|e| e.path())
            .collect();
        icons.sort();
        if let Some(p) = icons.into_iter().next() {
            return Some(Source::Image {
                path: p,
                mime: "image/jpeg",
            });
        }
    }
    let old = cache.join(format!("{appid}_icon.jpg"));
    old.is_file().then_some(Source::Image {
        path: old,
        mime: "image/jpeg",
    })
}

// --- macOS bundles -----------------------------------------------------------

fn bundle_icns(app: &Path) -> Option<PathBuf> {
    let res = app.join("Contents").join("Resources");
    #[cfg(target_os = "macos")]
    {
        let named = plist::Value::from_file(app.join("Contents").join("Info.plist"))
            .ok()
            .and_then(|v| {
                v.as_dictionary()?
                    .get("CFBundleIconFile")?
                    .as_string()
                    .map(str::to_string)
            });
        if let Some(n) = named.filter(|n| !n.is_empty() && !n.contains('/')) {
            let p = res.join(&n);
            let p = if p.extension().is_none() {
                p.with_extension("icns")
            } else {
                p
            };
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let app_icon = res.join("AppIcon.icns");
    if app_icon.is_file() {
        return Some(app_icon);
    }
    let mut all: Vec<PathBuf> = std::fs::read_dir(&res)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| ext_lower(p) == "icns")
        .collect();
    all.sort();
    all.into_iter().next()
}

/// The PNG inside an `.icns` file closest to (and not below) `want` pixels.
pub fn icns_best_png(data: &[u8], want: u32) -> Option<Vec<u8>> {
    if data.len() < 8 || &data[..4] != b"icns" {
        return None;
    }
    let mut best: Option<(u32, &[u8])> = None;
    let mut i = 8;
    while i + 8 <= data.len() {
        let ty = &data[i..i + 4];
        let len = u32::from_be_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        if len < 8 || i + len > data.len() {
            break;
        }
        let body = &data[i + 8..i + len];
        let px = match ty {
            b"icp4" => 16,
            b"icp5" | b"ic11" => 32,
            b"icp6" | b"ic12" => 64,
            b"ic07" => 128,
            b"ic08" | b"ic13" => 256,
            b"ic09" | b"ic14" => 512,
            b"ic10" => 1024,
            _ => 0,
        };
        if px > 0 && image_mime(body) == Some("image/png") {
            let better = match best {
                None => true,
                Some((b, _)) => {
                    // Prefer the smallest size that is still >= want.
                    (px >= want && (b < want || px < b)) || (b < want && px > b)
                }
            };
            if better {
                best = Some((px, body));
            }
        }
        i += len;
    }
    best.map(|(_, b)| b.to_vec())
}

// --- Extraction --------------------------------------------------------------

fn extract(path: &Path, index: i32) -> Option<Icon> {
    if ext_lower(path) == "icns" {
        let data = read_small(path, 16 * 1024 * 1024)?;
        if let Some(png) =
            icns_best_png(&data, ICON_PX).filter(|p| p.len() as u64 <= MAX_IMAGE_BYTES)
        {
            return Some(Icon {
                mime: "image/png",
                bytes: png,
            });
        }
        return platform::convert_icns(path).map(|bytes| Icon {
            mime: "image/png",
            bytes,
        });
    }
    platform::extract_resource(path, index, ICON_PX).map(|bytes| Icon {
        mime: "image/png",
        bytes,
    })
}

#[cfg(windows)]
mod platform {
    use crate::scan::platform::to_wide;
    use std::ffi::c_void;
    use std::path::Path;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::Shell::SHDefExtractIconW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    pub fn convert_icns(_path: &Path) -> Option<Vec<u8>> {
        None
    }

    /// Extract icon `index` of an `.exe`/`.dll`/`.ico` at `px` pixels as PNG.
    pub fn extract_resource(path: &Path, index: i32, px: u32) -> Option<Vec<u8>> {
        let w = to_wide(&path.to_string_lossy());
        let mut large: HICON = std::ptr::null_mut();
        let size = (px & 0xFFFF) | (16 << 16);
        let hr = unsafe {
            SHDefExtractIconW(w.as_ptr(), index, 0, &mut large, std::ptr::null_mut(), size)
        };
        if hr != 0 || large.is_null() {
            return None;
        }
        let rgba = unsafe { icon_rgba(large) };
        unsafe {
            DestroyIcon(large);
        }
        let (w, h, pixels) = rgba?;
        if pixels.chunks_exact(4).all(|p| p[3] == 0) {
            return None;
        }
        encode_png(w, h, &pixels)
    }

    /// The icon's pixels as straight RGBA.
    unsafe fn icon_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(icon, &mut ii) == 0 {
            return None;
        }
        let out = bitmap_rgba(&ii);
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        out
    }

    unsafe fn bitmap_rgba(ii: &ICONINFO) -> Option<(u32, u32, Vec<u8>)> {
        if ii.hbmColor.is_null() {
            return None; // monochrome icon
        }
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(
            ii.hbmColor,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bm as *mut BITMAP as *mut c_void,
        ) == 0
        {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if !(1..=1024).contains(&w) || !(1..=1024).contains(&h) {
            return None;
        }
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut mask_bi = bi;
        let len = (w * h * 4) as usize;
        let mut buf = vec![0u8; len];
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut ok = GetDIBits(
            dc,
            ii.hbmColor,
            0,
            h as u32,
            buf.as_mut_ptr() as *mut c_void,
            &mut bi,
            DIB_RGB_COLORS,
        ) == h;
        // Old icons have no alpha channel: take transparency from the mask.
        if ok && buf.chunks_exact(4).all(|p| p[3] == 0) {
            let mut mask = vec![0u8; len];
            ok = !ii.hbmMask.is_null()
                && GetDIBits(
                    dc,
                    ii.hbmMask,
                    0,
                    h as u32,
                    mask.as_mut_ptr() as *mut c_void,
                    &mut mask_bi,
                    DIB_RGB_COLORS,
                ) == h;
            if ok {
                for (p, m) in buf.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                    p[3] = if m[0] == 0 { 255 } else { 0 };
                }
            }
        }
        DeleteDC(dc);
        if !ok {
            return None;
        }
        for p in buf.chunks_exact_mut(4) {
            p.swap(0, 2); // BGRA -> RGBA
        }
        Some((w as u32, h as u32, buf))
    }

    pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().ok()?;
            wr.write_image_data(rgba).ok()?;
        }
        Some(out)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;

    pub fn extract_resource(_path: &Path, _index: i32, _px: u32) -> Option<Vec<u8>> {
        None
    }

    /// Older `.icns` files without PNG entries: let `sips` convert them.
    pub fn convert_icns(path: &Path) -> Option<Vec<u8>> {
        let out = std::env::temp_dir().join(format!(
            "roomy-icon-{}-{:?}.png",
            std::process::id(),
            std::thread::current().id()
        ));
        let ok = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "png", "-Z", &super::ICON_PX.to_string()])
            .arg(path)
            .arg("--out")
            .arg(&out)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let bytes = ok.then(|| std::fs::read(&out).ok()).flatten();
        let _ = std::fs::remove_file(&out);
        bytes.filter(|b| super::image_mime(b) == Some("image/png"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::path::Path;

    pub fn extract_resource(_path: &Path, _index: i32, _px: u32) -> Option<Vec<u8>> {
        None
    }

    pub fn convert_icns(_path: &Path) -> Option<Vec<u8>> {
        None
    }
}

// --- Store ---------------------------------------------------------------------

/// Icon lookups and their caches. Shared by the Tauri commands.
pub struct IconStore {
    dir: Option<PathBuf>,
    /// Icon key -> hints, registered when the UI receives entity summaries.
    sources: Mutex<HashMap<String, Vec<IconHint>>>,
    /// Icon key -> data URL (None: no icon, show the letter tile).
    mem: Mutex<HashMap<String, Option<Arc<str>>>>,
    pool: Option<rayon::ThreadPool>,
}

impl IconStore {
    /// `dir` is the on-disk cache folder (created on first write).
    pub fn new(dir: Option<PathBuf>) -> Self {
        if let Some(d) = &dir {
            prune(d, MAX_CACHE_FILES);
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("roomy-icons-{i}"))
            .start_handler(|_| lower_priority())
            .build()
            .ok();
        IconStore {
            dir,
            sources: Mutex::new(HashMap::new()),
            mem: Mutex::new(HashMap::new()),
            pool,
        }
    }

    /// Remember where the icon for `key` comes from.
    pub fn register(&self, key: &str, hints: &[IconHint]) {
        let mut s = self.sources.lock();
        if !s.contains_key(key) {
            s.insert(key.to_string(), hints.to_vec());
        }
    }

    /// Data URLs for the given keys (None when there's no icon).
    pub fn get_many(&self, keys: &[String]) -> HashMap<String, Option<String>> {
        let mut out: HashMap<String, Option<String>> = HashMap::new();
        let mut todo: Vec<(String, Vec<IconHint>)> = Vec::new();
        {
            let mem = self.mem.lock();
            let src = self.sources.lock();
            for k in keys.iter().take(MAX_KEYS_PER_CALL) {
                if out.contains_key(k) || todo.iter().any(|(t, _)| t == k) {
                    continue;
                }
                if let Some(v) = mem.get(k) {
                    out.insert(k.clone(), v.as_deref().map(str::to_string));
                } else if let Some(h) = src.get(k) {
                    todo.push((k.clone(), h.clone()));
                } else {
                    out.insert(k.clone(), None);
                }
            }
        }
        let work = |(k, h): &(String, Vec<IconHint>)| {
            (
                k.clone(),
                self.load(h).map(|i| Arc::<str>::from(i.data_url())),
            )
        };
        let done: Vec<(String, Option<Arc<str>>)> = match &self.pool {
            Some(p) => p.install(|| todo.par_iter().map(work).collect()),
            None => todo.iter().map(work).collect(),
        };
        let mut mem = self.mem.lock();
        for (k, v) in done {
            out.insert(k.clone(), v.as_deref().map(str::to_string));
            mem.insert(k, v);
        }
        out
    }

    /// The first icon any hint yields.
    pub fn load(&self, hints: &[IconHint]) -> Option<Icon> {
        hints.iter().flat_map(resolve).find_map(|s| self.read(&s))
    }

    fn read(&self, s: &Source) -> Option<Icon> {
        match s {
            Source::Image { path, .. } => {
                let bytes = read_small(path, MAX_IMAGE_BYTES)?;
                let mime = image_mime(&bytes)?;
                Some(Icon { mime, bytes })
            }
            Source::Resource { path, index } => self.extract_cached(path, *index),
        }
    }

    /// Extract with the on-disk cache in front. A failed extraction is
    /// remembered too, so it isn't retried until the file changes.
    fn extract_cached(&self, path: &Path, index: i32) -> Option<Icon> {
        let md = std::fs::metadata(path).ok()?;
        if !md.is_file() {
            return None;
        }
        let Some(dir) = &self.dir else {
            return extract(path, index);
        };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let id = blake3::hash(
            format!(
                "{CACHE_VERSION}|{}|{index}|{mtime}|{}|{ICON_PX}",
                path.to_string_lossy(),
                md.len()
            )
            .as_bytes(),
        )
        .to_hex()[..24]
            .to_string();
        let png = dir.join(format!("{id}.png"));
        let none = dir.join(format!("{id}.none"));
        if let Ok(bytes) = std::fs::read(&png) {
            if image_mime(&bytes) == Some("image/png") {
                return Some(Icon {
                    mime: "image/png",
                    bytes,
                });
            }
        }
        if none.exists() {
            return None;
        }
        let icon = extract(path, index);
        if std::fs::create_dir_all(dir).is_ok() {
            match &icon {
                Some(i) => write_atomic(&png, &i.bytes),
                None => write_atomic(&none, &[]),
            }
        }
        icon
    }
}

fn write_atomic(dest: &Path, bytes: &[u8]) {
    let tmp = dest.with_extension(format!(
        "tmp{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, dest).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Keep the cache folder bounded: clear Roomy's own cache files when there
/// are too many.
fn prune(dir: &Path, max: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let files: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    if files.len() <= max {
        return;
    }
    for f in files {
        let ext = ext_lower(&f);
        if ext == "png" || ext == "none" || ext.starts_with("tmp") {
            let _ = std::fs::remove_file(f);
        }
    }
}

fn lower_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// Standard base64 (RFC 4648) with padding.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        let a = vec![IconHint::ExeIn {
            dir: PathBuf::from("/x/A"),
        }];
        let b = vec![IconHint::ExeIn {
            dir: PathBuf::from("/x/B"),
        }];
        assert_eq!(key(&a), key(&a.clone()));
        assert_ne!(key(&a), key(&b));
        assert_eq!(key(&a).unwrap().len(), 16);
        assert_eq!(key(&[]), None);
    }

    #[test]
    fn picks_unplated_target_size_logo() {
        let files = names(&[
            "Square44x44Logo.scale-100.png",
            "Square44x44Logo.scale-200.png",
            "Square44x44Logo.targetsize-16_altform-unplated.png",
            "Square44x44Logo.targetsize-48_altform-unplated_contrast-black.png",
            "Square44x44Logo.targetsize-48_altform-unplated.png",
            "Square44x44Logo.targetsize-48.png",
            "Square44x44Logo.targetsize-256_altform-unplated.png",
            "Square150x150Logo.scale-200.png",
        ]);
        assert_eq!(
            pick_logo_variant("Square44x44Logo", "png", &files).as_deref(),
            Some("Square44x44Logo.targetsize-48_altform-unplated.png")
        );
        let plated = names(&["Logo.scale-100.png", "Logo.scale-200.png", "Logo.png"]);
        assert_eq!(
            pick_logo_variant("Logo", "png", &plated).as_deref(),
            Some("Logo.scale-200.png")
        );
        assert_eq!(
            pick_logo_variant("Logo", "png", &names(&["Logo.png"])).as_deref(),
            Some("Logo.png")
        );
        assert_eq!(
            pick_logo_variant("Logo", "png", &names(&["Other.png"])),
            None
        );
    }

    #[test]
    fn main_exe_prefers_the_named_app_over_helpers() {
        let c = |stem: &str, depth: u8, size: u64| ExeCandidate {
            stem: stem.into(),
            depth,
            size,
        };
        let list = vec![
            c("Update", 0, 2_000_000),
            c("unins000", 0, 3_000_000),
            c("Discord", 1, 150_000_000),
            c("crashpad_handler", 1, 900_000_000),
        ];
        assert_eq!(pick_main_exe("Discord", &list), Some(2));
        // Helpers are dropped; the rest stay as fallbacks in order.
        assert_eq!(rank_exes("Discord", &list), vec![2, 0]);
        // No name match: the biggest top-level program wins.
        let list = vec![
            c("r5apex", 0, 80_000_000),
            c("EasyAntiCheat_EOS_Setup", 0, 90_000_000),
            c("tool", 1, 200_000_000),
        ];
        assert_eq!(pick_main_exe("Apex Legends", &list), Some(0));
        assert_eq!(pick_main_exe("X", &[c("uninstall", 0, 1)]), None);
    }

    #[test]
    fn resolves_exe_in_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("Acme Paint");
        std::fs::create_dir_all(app.join("bin")).unwrap();
        std::fs::create_dir_all(app.join("locales")).unwrap();
        std::fs::write(app.join("uninstall.exe"), vec![0u8; 5000]).unwrap();
        std::fs::write(app.join("bin").join("AcmePaint.exe"), vec![0u8; 10]).unwrap();
        std::fs::write(app.join("locales").join("Acme Paint.exe"), vec![0u8; 10]).unwrap();
        let got = resolve(&IconHint::ExeIn { dir: app.clone() });
        assert_eq!(
            got,
            vec![Source::Resource {
                path: app.join("bin").join("AcmePaint.exe"),
                index: 0
            }]
        );
    }

    #[test]
    fn resolves_package_logo_from_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Pkg");
        std::fs::create_dir_all(root.join("Assets")).unwrap();
        std::fs::write(
            root.join("AppxManifest.xml"),
            r#"<Package><Properties><Logo>Assets\StoreLogo.png</Logo></Properties>
            <Applications><Application><uap:VisualElements DisplayName="X" Square44x44Logo="Assets\Square44x44Logo.png"/></Application></Applications></Package>"#,
        )
        .unwrap();
        for n in [
            "Square44x44Logo.scale-200.png",
            "Square44x44Logo.targetsize-64_altform-unplated.png",
            "StoreLogo.png",
        ] {
            std::fs::write(root.join("Assets").join(n), PNG_1PX).unwrap();
        }
        let got = resolve(&IconHint::Package { root: root.clone() });
        assert_eq!(
            got,
            vec![Source::Image {
                path: root
                    .join("Assets")
                    .join("Square44x44Logo.targetsize-64_altform-unplated.png"),
                mime: "image/png"
            }]
        );
        // A manifest pointing outside the package is ignored.
        std::fs::write(
            root.join("AppxManifest.xml"),
            r#"<Package><Properties><Logo>..\..\evil.png</Logo></Properties></Package>"#,
        )
        .unwrap();
        assert!(resolve(&IconHint::Package { root }).is_empty());
    }

    #[test]
    fn resolves_steam_library_icons() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("librarycache");
        std::fs::create_dir_all(cache.join("570")).unwrap();
        std::fs::write(cache.join("570").join("header.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
        let icon = cache
            .join("570")
            .join("0123456789abcdef0123456789abcdef01234567.jpg");
        std::fs::write(&icon, [0xFF, 0xD8, 0xFF]).unwrap();
        std::fs::write(cache.join("440_icon.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
        let hint = |id: &str| IconHint::Steam {
            cache: cache.clone(),
            appid: id.into(),
        };
        assert_eq!(
            resolve(&hint("570")),
            vec![Source::Image {
                path: icon,
                mime: "image/jpeg"
            }]
        );
        assert_eq!(
            resolve(&hint("440")),
            vec![Source::Image {
                path: cache.join("440_icon.jpg"),
                mime: "image/jpeg"
            }]
        );
        assert!(resolve(&hint("999")).is_empty());
        assert!(resolve(&hint("../570")).is_empty());
    }

    #[test]
    fn icns_png_entry_closest_to_wanted_size() {
        let entry = |ty: &[u8; 4], body: &[u8]| {
            let mut v = ty.to_vec();
            v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
            v.extend_from_slice(body);
            v
        };
        let mut png128 = PNG_1PX.to_vec();
        png128.push(128);
        let mut png1024 = PNG_1PX.to_vec();
        png1024.push(4);
        let mut body = Vec::new();
        body.extend(entry(b"ic10", &png1024));
        body.extend(entry(b"is32", &[1, 2, 3]));
        body.extend(entry(b"ic07", &png128));
        body.extend(entry(b"ic11", PNG_1PX));
        let mut file = b"icns".to_vec();
        file.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
        file.extend(body);
        assert_eq!(icns_best_png(&file, 64), Some(png128));
        assert_eq!(icns_best_png(b"nope", 64), None);
    }

    #[test]
    fn store_serves_images_and_caches_in_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let img = tmp.path().join("logo.png");
        std::fs::write(&img, PNG_1PX).unwrap();
        let store = IconStore::new(Some(tmp.path().join("cache")));
        let hints = vec![
            IconHint::File {
                path: tmp.path().join("missing.exe"),
                index: 0,
            },
            IconHint::File {
                path: img.clone(),
                index: 0,
            },
        ];
        let k = key(&hints).unwrap();
        store.register(&k, &hints);
        let got = store.get_many(&[k.clone(), "unknown".into()]);
        let url = got[&k].clone().unwrap();
        assert!(url.starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert_eq!(got["unknown"], None);
        // Served from memory even after the file is gone.
        std::fs::remove_file(&img).unwrap();
        assert_eq!(
            store.get_many(std::slice::from_ref(&k))[&k].as_deref(),
            Some(url.as_str())
        );
    }

    #[test]
    fn non_images_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake.png");
        std::fs::write(&fake, b"not a png").unwrap();
        let store = IconStore::new(None);
        assert_eq!(
            store.load(&[IconHint::File {
                path: fake,
                index: 0
            }]),
            None
        );
    }

    #[test]
    fn failed_extractions_are_remembered_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("NotAnIcon.exe");
        std::fs::write(&exe, b"plain text, no icon resources").unwrap();
        let cache = tmp.path().join("cache");
        let store = IconStore::new(Some(cache.clone()));
        let hints = [IconHint::File {
            path: exe,
            index: 0,
        }];
        assert_eq!(store.load(&hints), None);
        let files: Vec<_> = std::fs::read_dir(&cache).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        assert!(files[0].path().extension().unwrap() == "none");
    }

    #[test]
    fn prune_clears_an_oversized_cache() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(tmp.path().join(format!("{i}.png")), b"x").unwrap();
        }
        std::fs::write(tmp.path().join("keep.txt"), b"x").unwrap();
        prune(tmp.path(), 10);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 6);
        prune(tmp.path(), 3);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn extracts_and_caches_windows_icons() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let shell32 = Path::new(&root).join("System32").join("shell32.dll");
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let store = IconStore::new(Some(cache.clone()));
        let hints = [IconHint::File {
            path: shell32,
            index: 3,
        }];
        let icon = store.load(&hints).expect("shell32 has icons");
        assert_eq!(icon.mime, "image/png");
        assert_eq!(image_mime(&icon.bytes), Some("image/png"));
        let cached: Vec<_> = std::fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(cached.len(), 1);
        assert_eq!(std::fs::read(&cached[0]).unwrap(), icon.bytes);
        // A second store (next launch) reads the same bytes from disk.
        let again = IconStore::new(Some(cache)).load(&hints).unwrap();
        assert_eq!(again, icon);
    }
}
