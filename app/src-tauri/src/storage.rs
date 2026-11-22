//! The Storage view: every byte of the scan attributed to exactly one thing
//! (an app, a game, a project, a system component, a cache, a media type …).
//!
//! How it works:
//! 1. Build **claims**: tree nodes that belong to an entity (an app's install
//!    folder, its AppData folders, a game folder, the Windows folder, the
//!    Downloads folder, a detected project root …).
//! 2. Walk the scanned tree once. Each node inherits the owner of its nearest
//!    claimed ancestor (the most specific claim wins). Each file's size is
//!    added to exactly one entity, so nothing is counted twice and the total
//!    equals the scanned total (plus the unreadable gap, which is its own
//!    System entity).
//! 3. Files nobody claims go to a file-type entity (photos, videos, music,
//!    documents) or to an "Other" entity named after their top folder.

use crate::catalog::{
    base_name, norm, norm_publisher, AppKind, AppRecord, Catalog, IconHint, Uninstall,
};
use crate::tree::*;
use crate::views::{category_of, find_by_path};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Apps,
    Games,
    Projects,
    Media,
    Documents,
    Downloads,
    System,
    Caches,
    Other,
}

pub const ALL_CATEGORIES: [Category; 9] = [
    Category::Apps,
    Category::Games,
    Category::Projects,
    Category::Media,
    Category::Documents,
    Category::Downloads,
    Category::System,
    Category::Caches,
    Category::Other,
];

pub const B_FILES: u8 = 0;
pub const B_DATA: u8 = 1;
pub const B_CACHE: u8 = 2;
pub const B_BUILD: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntityKind {
    App,
    Game,
    Project,
    Folder,
    FileType,
    System,
    Cache,
    Hidden,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub node: u32,
    pub bucket: u8,
    /// Bytes attributed through this location (nested claims excluded).
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub id: u32,
    pub category: Category,
    pub kind: EntityKind,
    pub name: String,
    pub detail: Option<String>,
    pub size: u64,
    /// files / data / cache / build-and-deps
    pub buckets: [u64; 4],
    pub files: u64,
    pub locations: Vec<Location>,
    #[serde(skip)]
    pub cache_dirs: Vec<u32>,
    #[serde(skip)]
    pub junk_dirs: Vec<u32>,
    /// Bytes clearing the cache folders would really free (hard links aware).
    pub reclaim_cache: u64,
    /// Bytes removing build output/deps would really free (hard links aware).
    pub reclaim_build: u64,
    pub uninstall: Option<Uninstall>,
    pub mtime: i64,
    /// Where this app's or game's icon may be found (read lazily by `icons`).
    #[serde(skip)]
    pub icons: Vec<IconHint>,
}

impl Entity {
    fn new(id: u32, category: Category, kind: EntityKind, name: impl Into<String>) -> Self {
        Entity {
            id,
            category,
            kind,
            name: name.into(),
            detail: None,
            size: 0,
            buckets: [0; 4],
            files: 0,
            locations: Vec::new(),
            cache_dirs: Vec::new(),
            junk_dirs: Vec::new(),
            reclaim_cache: 0,
            reclaim_build: 0,
            uninstall: None,
            mtime: 0,
            icons: Vec::new(),
        }
    }

    pub fn reclaimable(&self) -> u64 {
        self.reclaim_cache + self.reclaim_build
    }
}

/// Well-known locations, injectable for tests.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub home: Option<PathBuf>,
    pub app_data_roots: Vec<PathBuf>, // Roaming, Local, LocalLow, ProgramData, ~/Library/…
    pub program_files: Vec<PathBuf>,  // Program Files, (x86), Local\Programs
    pub system_root: Option<PathBuf>,
    pub system_drive: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    pub pictures: Option<PathBuf>,
    pub videos: Option<PathBuf>,
    pub music: Option<PathBuf>,
    pub onedrive: Option<PathBuf>,
    pub temp: Option<PathBuf>,
}

impl Env {
    pub fn system() -> Env {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let home = dirs::home_dir();
        let mut app_data_roots = Vec::new();
        let mut program_files = Vec::new();
        #[cfg(windows)]
        {
            for k in ["APPDATA", "LOCALAPPDATA", "ProgramData"] {
                if let Some(p) = var(k) {
                    app_data_roots.push(p);
                }
            }
            if let Some(h) = &home {
                app_data_roots.push(h.join(r"AppData\LocalLow"));
            }
            for k in ["ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(p) = var(k) {
                    program_files.push(p);
                }
            }
            if let Some(l) = var("LOCALAPPDATA") {
                program_files.push(l.join("Programs"));
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(h) = &home {
            for s in [
                "Library/Application Support",
                "Library/Caches",
                "Library/Containers",
                "Library/Group Containers",
            ] {
                app_data_roots.push(h.join(s));
            }
            program_files.push(PathBuf::from("/Applications"));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(h) = &home {
            app_data_roots.push(h.join(".config"));
            app_data_roots.push(h.join(".local/share"));
            app_data_roots.push(h.join(".cache"));
        }
        let onedrive = var("OneDrive").or_else(|| home.as_ref().map(|h| h.join("OneDrive")));
        Env {
            app_data_roots,
            program_files,
            system_root: var("SystemRoot"),
            system_drive: var("SystemDrive")
                .map(|d| PathBuf::from(format!("{}\\", d.to_string_lossy()))),
            downloads: dirs::download_dir(),
            documents: dirs::document_dir(),
            desktop: dirs::desktop_dir(),
            pictures: dirs::picture_dir(),
            videos: dirs::video_dir(),
            music: dirs::audio_dir(),
            onedrive,
            temp: Some(std::env::temp_dir()),
            home,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotal {
    pub category: Category,
    pub size: u64,
    pub count: u32,
}

pub struct Storage {
    pub entities: Vec<Entity>,
    pub categories: Vec<CategoryTotal>,
    pub total: u64,
    pub generation: u64,
}

/// Folder names that are regenerable caches when found inside an app's
/// *data* folders (Chromium/Electron/Firefox/Steam conventions). These are
/// the only places "Clear cache" may delete; install folders never qualify.
const CACHE_DIR_NAMES: &[&str] = &[
    "cache",
    "code cache",
    "gpucache",
    "cache2",
    "shadercache",
    "grshadercache",
    "dawncache",
    "dawngraphitecache",
    "dawnwebgpucache",
    "cacheddata",
    "cachedextensionvsixs",
    "htmlcache",
];

const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "package.json",
    "cargo.toml",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "cmakelists.txt",
    "composer.json",
    "gemfile",
    "pubspec.yaml",
    "requirements.txt",
    "deno.json",
    "mix.exs",
    "project.godot",
    "*.sln",
    "*.csproj",
    "*.uproject",
    "*.xcodeproj",
];

const JUNK_DIRS: &[(&str, &[&str])] = &[
    ("node_modules", &["package.json"]),
    ("target", &["cargo.toml"]),
    (".next", &["package.json"]),
    (".nuxt", &["package.json"]),
    (".svelte-kit", &["package.json"]),
    (".turbo", &["package.json", "turbo.json"]),
    (".parcel-cache", &["package.json"]),
    (".angular", &["angular.json"]),
    (
        ".gradle",
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
    ),
    (
        ".venv",
        &["pyproject.toml", "requirements.txt", "setup.py", "pipfile"],
    ),
    (
        "venv",
        &["pyproject.toml", "requirements.txt", "setup.py", "pipfile"],
    ),
    ("__pycache__", &["*"]),
    ("obj", &["*.csproj", "*.fsproj", "*.vbproj"]),
    ("intermediate", &["*.uproject"]),
    ("deriveddatacache", &["*.uproject"]),
    (".godot", &["project.godot"]),
];

const NOT_PROJECT_DIRS: &[&str] = &[
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
    "scoop",
    "system volume information",
    ".local",
    ".gradle",
    ".m2",
    "go",
    ".git",
    "site-packages",
    ".pnpm-store",
    ".bun",
    "steamapps",
    "xboxgames",
    ".vscode-server",
];

/// True if `inner` is strictly inside `outer` (case-insensitive on Windows).
fn path_contains(outer: &Path, inner: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| {
                let s = c.as_os_str().to_string_lossy().replace('/', "\\");
                if cfg!(windows) {
                    s.to_lowercase()
                } else {
                    s
                }
            })
            .filter(|s| !s.is_empty() && s != "\\")
            .collect()
    };
    let (o, i) = (norm(outer), norm(inner));
    !o.is_empty() && i.len() > o.len() && i[..o.len()] == o[..]
}

/// Path components that mark an installed or system location.
const INSTALLED_AREA_NAMES: &[&str] = &[
    "appdata",
    "program files",
    "program files (x86)",
    "programdata",
    "windows",
    "node_modules",
    "library",
    ".cargo",
    ".rustup",
    "site-packages",
    "steamapps",
    "xboxgames",
    "windowsapps",
    "$recycle.bin",
    "applications",
    ".nvm",
    ".npm",
];

fn same_path(a: &Path, b: &Path) -> bool {
    let k = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.trim_end_matches('\\').to_string();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    k(a) == k(b)
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn matches_marker(marker: &str, name: &str) -> bool {
    let n = lower(name);
    match marker.strip_prefix('*') {
        Some(ext) => n.ends_with(ext),
        None => n == marker,
    }
}

/// Builder state shared by the claim phase.
struct Builder<'a> {
    tree: &'a Tree,
    entities: Vec<Entity>,
    claims: HashMap<u32, (u32, u8, u32)>, // node -> (entity, bucket, location index)
    /// The most specific claim that contains the scanned folder itself
    /// (scanning a folder inside Downloads, an app, a project …).
    root_claim: Option<(usize, u32, u8)>,
}

impl<'a> Builder<'a> {
    fn entity(&mut self, category: Category, kind: EntityKind, name: impl Into<String>) -> u32 {
        let id = self.entities.len() as u32;
        self.entities.push(Entity::new(id, category, kind, name));
        id
    }

    fn node(&self, p: &Path) -> Option<u32> {
        find_by_path(self.tree, p).or_else(|| {
            // macOS scans of the data volume see /Users at /System/Volumes/Data/Users.
            let root = Path::new(&self.tree.root_path);
            if root.starts_with("/System/Volumes/Data") && p.is_absolute() {
                let rel = p.strip_prefix("/").ok()?;
                find_by_path(self.tree, &root.join(rel))
            } else {
                None
            }
        })
    }

    /// Claim a node for an entity. Returns false if it was already claimed
    /// (first claim of a node wins) or isn't in the scan.
    fn claim_node(&mut self, node: u32, entity: u32, bucket: u8) -> bool {
        if self.claims.contains_key(&node) || self.tree.get(node).is_none() {
            return false;
        }
        let loc = self.entities[entity as usize].locations.len() as u32;
        self.entities[entity as usize].locations.push(Location {
            node,
            bucket,
            bytes: 0,
        });
        self.claims.insert(node, (entity, bucket, loc));
        true
    }

    fn claim(&mut self, p: &Path, entity: u32, bucket: u8) -> bool {
        match self.node(p) {
            Some(0) => {
                self.offer_root(p.components().count(), entity, bucket);
                false
            }
            Some(n) => self.claim_node(n, entity, bucket),
            None => {
                if path_contains(p, Path::new(&self.tree.root_path)) {
                    self.offer_root(p.components().count(), entity, bucket);
                }
                false
            }
        }
    }

    /// Offer a claim on the scanned folder itself; the deepest offer wins.
    fn offer_root(&mut self, depth: usize, entity: u32, bucket: u8) {
        if self.root_claim.map(|(d, _, _)| depth > d).unwrap_or(true) {
            self.root_claim = Some((depth, entity, bucket));
        }
    }

    fn is_claimed_or_under_claim(&self, node: u32) -> bool {
        let mut cur = node;
        loop {
            if self.claims.contains_key(&cur) {
                return true;
            }
            let p = self.tree.nodes[cur as usize].parent;
            if p == NONE {
                return false;
            }
            cur = p;
        }
    }

    fn child_dirs(&self, node: u32) -> Vec<u32> {
        self.tree
            .children(node)
            .filter(|&c| self.tree.nodes[c as usize].kind == KIND_DIR)
            .collect()
    }
}

/// Paths that must never be claimed wholesale by an app (bad registry data).
fn too_generic(p: &Path, env: &Env) -> bool {
    let key = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let k = key(p);
    if k.len() <= 3 || p.components().count() <= 2 {
        return true;
    }
    let mut roots: Vec<&PathBuf> = Vec::new();
    roots.extend(env.home.iter());
    roots.extend(env.app_data_roots.iter());
    roots.extend(env.program_files.iter());
    roots.extend(env.system_root.iter());
    roots.extend(env.downloads.iter());
    roots.extend(env.documents.iter());
    roots.extend(env.desktop.iter());
    roots.iter().any(|r| {
        let rk = key(r);
        rk == k || rk.starts_with(&format!("{k}\\")) || rk.starts_with(&format!("{k}/"))
    }) || env
        .system_root
        .as_ref()
        .map(|s| k.starts_with(&key(s)))
        .unwrap_or(false)
}

const GENERIC_KEYS: &[&str] = &[
    "app",
    "apps",
    "data",
    "common",
    "shared",
    "update",
    "updater",
    "microsoft",
    "windows",
    "program",
    "programs",
    "tools",
    "bin",
    "system",
    "temp",
    "cache",
    "user",
    "users",
    "default",
    "local",
    "roaming",
    "setup",
    "install",
    "uninstall",
    "packages",
    "runtime",
    "service",
    "services",
    "helper",
    "client",
    "x64",
    "x86",
    "launcher",
];

fn app_keys(app: &AppRecord) -> Vec<String> {
    let mut keys = vec![norm(&app.name), norm(&base_name(&app.name))];
    if let Some(p) = &app.publisher {
        let pubn = norm_publisher(p);
        let b = norm(&base_name(&app.name));
        if let Some(rest) = b.strip_prefix(&pubn) {
            keys.push(rest.to_string());
        }
    }
    for d in &app.install_dirs {
        if let Some(n) = d.file_name() {
            keys.push(norm(&n.to_string_lossy()));
        }
    }
    for a in &app.aliases {
        keys.push(norm(a));
    }
    // A few well-known folder names that don't match the display name.
    let n = norm(&app.name);
    let extra: &[&str] = if n.contains("visualstudiocode") {
        &["code"]
    } else if n.contains("bravebrowser") || n == "brave" {
        &["bravesoftware"]
    } else if n.contains("dockerdesktop") {
        &["docker"]
    } else if n.contains("epicgameslauncher") {
        &["epicgameslauncher", "epicgames"]
    } else if n.contains("microsoftteams") {
        &["teams"]
    } else if n.contains("whatsapp") {
        &["whatsapp"]
    } else {
        &[]
    };
    keys.extend(extra.iter().map(|s| s.to_string()));
    keys.retain(|k| k.len() >= 3 && !GENERIC_KEYS.contains(&k.as_str()));
    keys.sort();
    keys.dedup();
    keys
}

pub fn attribute(tree: &Tree, catalog: &Catalog, env: &Env, is_volume: bool) -> Storage {
    let mut b = Builder {
        tree,
        entities: Vec::new(),
        claims: HashMap::new(),
        root_claim: None,
    };

    // --- System (claimed first so nothing else can take these) ----------
    let bin = b.entity(Category::Caches, EntityKind::Cache, "Recycle Bin");
    b.entities[bin as usize].detail =
        Some("Files you deleted. They still use space until the bin is emptied.".into());
    for c in tree.children(0) {
        let k = tree.nodes[c as usize].kind;
        if k == KIND_GAP {
            let e = b.entity(
                Category::System,
                EntityKind::Hidden,
                "Restore points, reserved & unreadable",
            );
            b.entities[e as usize].detail =
                Some("Space the system reports as used that no folder listing shows".into());
            b.claim_node(c, e, B_FILES);
            for part in tree.children(c) {
                if tree.name(part).starts_with("Recycle Bin") {
                    b.claim_node(part, bin, B_FILES);
                } else if tree.name(part).starts_with("NTFS") {
                    let m = b.entity(
                        Category::System,
                        EntityKind::Hidden,
                        "NTFS file table ($MFT)",
                    );
                    b.entities[m as usize].detail =
                        Some("The index of every file on the drive".into());
                    b.claim_node(part, m, B_FILES);
                }
            }
        }
    }
    if let Some(win) = env.system_root.clone() {
        let w = b.entity(Category::System, EntityKind::System, "Windows");
        b.entities[w as usize].detail = Some("The operating system".into());
        b.claim(&win, w, B_FILES);
        let sxs = b.entity(
            Category::System,
            EntityKind::System,
            "Windows component store (WinSxS)",
        );
        b.entities[sxs as usize].detail = Some("Shared system components and update backups. Much of it is hard-linked into Windows, so it's counted once.".into());
        b.claim(&win.join("WinSxS"), sxs, B_FILES);
        let installer = b.entity(
            Category::System,
            EntityKind::System,
            "Windows Installer cache",
        );
        b.entities[installer as usize].detail =
            Some("Needed to repair and uninstall programs".into());
        b.claim(&win.join("Installer"), installer, B_FILES);
    }
    if let Some(drive) = env.system_drive.clone() {
        for (file, name, detail) in [
            (
                "pagefile.sys",
                "Page file",
                "Virtual memory managed by Windows",
            ),
            (
                "hiberfil.sys",
                "Hibernation file",
                "Saves memory when the PC hibernates or fast-starts",
            ),
            (
                "swapfile.sys",
                "Swap file",
                "Used by Windows to suspend Store apps",
            ),
        ] {
            let e = b.entity(Category::System, EntityKind::System, name);
            b.entities[e as usize].detail = Some(detail.into());
            b.claim(&drive.join(file), e, B_FILES);
        }
        let svi = b.entity(
            Category::System,
            EntityKind::System,
            "Restore points & shadow copies",
        );
        b.entities[svi as usize].detail = Some("System Volume Information".into());
        b.claim(&drive.join("System Volume Information"), svi, B_FILES);
        let rec = b.entity(Category::System, EntityKind::System, "Recovery & boot");
        for d in [
            "Recovery",
            "$WinREAgent",
            "Boot",
            "EFI",
            "$SysReset",
            "$Windows.~BT",
            "$Windows.~WS",
        ] {
            b.claim(&drive.join(d), rec, B_FILES);
        }
        b.claim(&drive.join("$Recycle.Bin"), bin, B_FILES);
    }
    let shared = b.entity(
        Category::System,
        EntityKind::System,
        "Shared system components",
    );
    b.entities[shared as usize].detail = Some(
        "Common Files, Windows Defender, installer caches and other parts many apps rely on".into(),
    );
    for pf in env.program_files.clone() {
        for d in [
            "Common Files",
            "Windows Defender",
            "Windows NT",
            "Windows Defender Advanced Threat Protection",
            "WindowsApps",
            "ModifiableWindowsApps",
            "Microsoft Update Health Tools",
            "Windows Security",
        ] {
            b.claim(&pf.join(d), shared, B_FILES);
        }
    }
    for root in env.app_data_roots.clone() {
        if root
            .file_name()
            .map(|n| n.eq_ignore_ascii_case("ProgramData"))
            .unwrap_or(false)
        {
            for d in [
                "Microsoft",
                "Package Cache",
                "Packages",
                "regid.1991-06.com.microsoft",
                "USOShared",
                "USOPrivate",
            ] {
                b.claim(&root.join(d), shared, B_FILES);
            }
        }
    }

    // --- Apps and games from the catalog ---------------------------------
    let store_system = b.entity(
        Category::System,
        EntityKind::System,
        "Built-in Windows apps & frameworks",
    );
    let mut key_index: HashMap<String, u32> = HashMap::new();
    let mut publisher_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut app_entities: Vec<u32> = Vec::new();
    for app in &catalog.apps {
        let (cat, kind) = match app.kind {
            AppKind::Game => (Category::Games, EntityKind::Game),
            AppKind::App => (Category::Apps, EntityKind::App),
            AppKind::System => (Category::System, EntityKind::System),
        };
        let e = if app.kind == AppKind::System {
            store_system
        } else {
            let e = b.entity(cat, kind, app.name.clone());
            let ent = &mut b.entities[e as usize];
            ent.detail = app
                .publisher
                .clone()
                .or_else(|| (app.source != "Windows").then(|| app.source.to_string()));
            ent.uninstall = app.uninstall.clone();
            ent.icons = app.icons.clone();
            e
        };
        for d in &app.install_dirs {
            if !too_generic(d, env) {
                b.claim(d, e, B_FILES);
            }
        }
        for d in &app.data_dirs {
            b.claim(d, e, B_DATA);
        }
        for d in &app.cache_dirs {
            if !valid_catalog_cache(d) {
                continue;
            }
            if let Some(n) = b.node(d) {
                if b.claim_node(n, e, B_CACHE) {
                    b.entities[e as usize].cache_dirs.push(n);
                }
            }
        }
        if app.kind != AppKind::System {
            app_entities.push(e);
            for k in app_keys(app) {
                key_index.entry(k).or_insert(e);
            }
            if let Some(p) = &app.publisher {
                let pk = norm_publisher(p);
                if pk.len() >= 3 && !GENERIC_KEYS.contains(&pk.as_str()) {
                    publisher_index.entry(pk).or_default().push(e);
                }
            }
        }
    }

    // Xbox / Game Pass installs: <drive>\XboxGames\<Game>
    for c in tree.children(0) {
        if tree.nodes[c as usize].kind == KIND_DIR && tree.name(c).eq_ignore_ascii_case("XboxGames")
        {
            for g in b.child_dirs(c) {
                if b.claims.contains_key(&g) {
                    continue;
                }
                let e = b.entity(Category::Games, EntityKind::Game, tree.name(g).to_string());
                b.entities[e as usize].detail = Some("Xbox app".into());
                b.entities[e as usize].uninstall = Some(Uninstall::Settings);
                let dir = tree.path(g);
                b.entities[e as usize].icons = vec![
                    IconHint::Package {
                        root: dir.join("Content"),
                    },
                    IconHint::Package { root: dir.clone() },
                    IconHint::ExeIn { dir },
                ];
                b.claim_node(g, e, B_FILES);
            }
        }
    }

    // --- Caches & temp (before app data so they stay separate from apps) --
    let cache_specs: Vec<(&str, Vec<PathBuf>)> = cache_locations(env);
    for (name, paths) in cache_specs {
        let mut e: Option<u32> = None;
        for p in paths {
            let Some(n) = b.node(&p) else { continue };
            if b.is_claimed_or_under_claim(n) {
                continue;
            }
            let id = *e.get_or_insert_with(|| b.entity(Category::Caches, EntityKind::Cache, name));
            b.claim_node(n, id, B_CACHE);
        }
    }

    // --- App data folders matched by name / publisher --------------------
    for root in env.app_data_roots.clone() {
        let Some(rn) = b.node(&root) else { continue };
        for f in b.child_dirs(rn) {
            if b.claims.contains_key(&f) {
                continue;
            }
            let fname = tree.name(f).to_string();
            let fk = norm(&fname);
            if let Some(&e) = key_index.get(&fk) {
                b.claim_node(f, e, B_DATA);
                continue;
            }
            // Publisher folder (Google\Chrome, Microsoft\Edge, JetBrains\…).
            let pubs = publisher_index.get(&fk).cloned().unwrap_or_default();
            let is_ms = fk == "microsoft";
            if pubs.is_empty() && !is_ms {
                continue;
            }
            let mut matched_any = false;
            for g in b.child_dirs(f) {
                if b.claims.contains_key(&g) {
                    continue;
                }
                let gk = norm(tree.name(g));
                let hit = key_index.get(&gk).copied().or_else(|| {
                    // "Chrome" under "Google" for app "Google Chrome".
                    key_index.get(&format!("{fk}{gk}")).copied()
                });
                if let Some(e) = hit {
                    if is_ms || pubs.contains(&e) || pubs.is_empty() {
                        b.claim_node(g, e, B_DATA);
                        matched_any = true;
                    }
                }
            }
            if !matched_any && pubs.len() == 1 && !is_ms {
                b.claim_node(f, pubs[0], B_DATA);
            }
        }
    }

    // --- User folders -------------------------------------------------------
    let user_folders: [(&Option<PathBuf>, Category, &str); 7] = [
        (&env.downloads, Category::Downloads, "Downloads"),
        (&env.documents, Category::Documents, "Documents"),
        (&env.desktop, Category::Documents, "Desktop"),
        (&env.onedrive, Category::Documents, "OneDrive"),
        (&env.pictures, Category::Media, "Pictures"),
        (&env.videos, Category::Media, "Videos"),
        (&env.music, Category::Media, "Music"),
    ];
    // Projects inside Documents/Desktop should still show as projects, so
    // detect them before claiming the user folders.
    let mut project_roots: Vec<u32> = Vec::new();
    // The home folder and anything containing a well-known location can't be a
    // project, even if a stray package.json sits there.
    let mut not_roots: HashSet<u32> = HashSet::new();
    let special: Vec<PathBuf> = [
        &env.home,
        &env.documents,
        &env.downloads,
        &env.desktop,
        &env.onedrive,
    ]
    .into_iter()
    .flatten()
    .cloned()
    .chain(env.app_data_roots.iter().cloned())
    .chain(env.program_files.iter().cloned())
    .collect();
    for p in special {
        if let Some(mut n) = b.node(&p) {
            loop {
                not_roots.insert(n);
                let parent = tree.nodes[n as usize].parent;
                if parent == NONE {
                    break;
                }
                n = parent;
            }
        }
    }
    let scan_root = PathBuf::from(&tree.root_path);
    // Scanning inside an app, game, system, cache or installed-package area:
    // nothing in it is the user's project, whatever marker files it holds.
    let installed_area = env
        .app_data_roots
        .iter()
        .chain(env.program_files.iter())
        .chain(env.system_root.iter())
        .any(|r| same_path(r, &scan_root) || path_contains(r, &scan_root))
        || scan_root.components().any(|c| {
            INSTALLED_AREA_NAMES.contains(&c.as_os_str().to_string_lossy().to_lowercase().as_str())
        })
        || b.root_claim
            .map(|(_, e, _)| {
                matches!(
                    b.entities[e as usize].kind,
                    EntityKind::App
                        | EntityKind::Game
                        | EntityKind::System
                        | EntityKind::Cache
                        | EntityKind::Hidden
                )
            })
            .unwrap_or(false);
    if !installed_area {
        find_projects(&b, 0, &mut project_roots, &not_roots);
    }
    if !installed_area && !not_roots.contains(&0) && !b.claims.contains_key(&0) {
        let root_path = PathBuf::from(&tree.root_path);
        let (proj_name, proj_path) = if has_marker(tree, 0) {
            (Some(tree.name(0).to_string()), Some(root_path.clone()))
        } else {
            let stop: Vec<PathBuf> = [
                &env.home,
                &env.documents,
                &env.downloads,
                &env.desktop,
                &env.onedrive,
            ]
            .into_iter()
            .flatten()
            .cloned()
            .chain(env.app_data_roots.iter().cloned())
            .chain(env.program_files.iter().cloned())
            .collect();
            match enclosing_project(&root_path, &stop) {
                Some(p) => (
                    p.file_name().map(|n| n.to_string_lossy().into_owned()),
                    Some(p),
                ),
                None => (None, None),
            }
        };
        if let (Some(name), Some(pp)) = (proj_name, proj_path) {
            let name = Path::new(&name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(name);
            let e = b.entity(Category::Projects, EntityKind::Project, name);
            b.entities[e as usize].detail = Some(project_kinds(tree, 0));
            b.entities[e as usize].mtime = tree.nodes[0].mtime;
            // Deeper than any known folder that might also contain the root.
            b.offer_root(pp.components().count() + 1000, e, B_FILES);
        }
    }
    for p in project_roots {
        let name = tree.name(p).to_string();
        let e = b.entity(Category::Projects, EntityKind::Project, name);
        let kinds = project_kinds(tree, p);
        b.entities[e as usize].detail = Some(kinds);
        b.entities[e as usize].mtime = tree.nodes[p as usize].mtime;
        b.claim_node(p, e, B_FILES);
    }
    for (p, cat, name) in user_folders {
        if let Some(p) = p {
            let e = b.entity(cat, EntityKind::Folder, name);
            b.claim(p, e, B_FILES);
        }
    }

    if let Some((_, e, bk)) = b.root_claim {
        b.claim_node(0, e, bk);
    }

    // --- Walk once and attribute every node ------------------------------
    attribute_walk(&mut b, env, is_volume)
}

fn cache_locations(env: &Env) -> Vec<(&'static str, Vec<PathBuf>)> {
    let mut v: Vec<(&'static str, Vec<PathBuf>)> = Vec::new();
    let local = env
        .app_data_roots
        .iter()
        .find(|p| p.ends_with("Local"))
        .cloned();
    let home = env.home.clone();
    let j = |base: &Option<PathBuf>, rel: &str| base.as_ref().map(|b| b.join(rel));
    let mut temp = vec![];
    if let Some(t) = &env.temp {
        temp.push(t.clone());
    }
    if let Some(l) = &local {
        temp.push(l.join("Temp"));
    }
    if let Some(w) = &env.system_root {
        temp.push(w.join("Temp"));
    }
    v.push(("Temporary files", temp));
    v.push((
        "Thumbnail & icon cache",
        local
            .iter()
            .map(|l| l.join(r"Microsoft\Windows\Explorer"))
            .collect(),
    ));
    v.push((
        "Crash dumps & error reports",
        [j(&local, "CrashDumps"), j(&local, r"Microsoft\Windows\WER")]
            .into_iter()
            .flatten()
            .chain(env.system_root.iter().flat_map(|w| {
                [
                    w.join("Minidump"),
                    w.join("LiveKernelReports"),
                    w.join("MEMORY.DMP"),
                ]
            }))
            .collect(),
    ));
    v.push((
        "Graphics shader caches",
        [
            j(&local, "D3DSCache"),
            j(&local, r"NVIDIA\DXCache"),
            j(&local, r"NVIDIA\GLCache"),
            j(&local, r"AMD\DxCache"),
            j(&local, r"AMD\DxcCache"),
            j(&local, r"Intel\ShaderCache"),
        ]
        .into_iter()
        .flatten()
        .collect(),
    ));
    v.push((
        "Windows Update downloads",
        env.system_root
            .iter()
            .flat_map(|w| [w.join(r"SoftwareDistribution\Download"), w.join(r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache")])
            .collect(),
    ));
    v.push((
        "Package manager caches",
        [
            j(&local, "npm-cache"),
            j(&local, "pnpm-cache"),
            j(&local, r"pnpm\store"),
            j(&local, "Yarn"),
            j(&local, r"pip\cache"),
            j(&local, r"uv\cache"),
            j(&local, "go-build"),
            j(&local, r"NuGet\v3-cache"),
            j(&local, r"NuGet\plugins-cache"),
            j(&local, r"Composer"),
            j(&home, ".npm"),
            j(&home, ".cargo/registry"),
            j(&home, ".cargo/git"),
            j(&home, ".gradle/caches"),
            j(&home, ".nuget/packages"),
            j(&home, ".m2/repository"),
            j(&home, ".bun/install/cache"),
            j(&home, "Library/Caches"),
            j(&home, ".cache"),
        ]
        .into_iter()
        .flatten()
        .collect(),
    ));
    v
}

/// Catalog cache folders Roomy may clear: only Steam's per-game shader
/// cache, `<library>/steamapps/shadercache/<numeric appid>`.
fn valid_catalog_cache(p: &Path) -> bool {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = p
        .parent()
        .and_then(|x| x.file_name())
        .map(|n| n.to_string_lossy().to_lowercase());
    let grand = p
        .parent()
        .and_then(|x| x.parent())
        .and_then(|x| x.file_name())
        .map(|n| n.to_string_lossy().to_lowercase());
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_digit())
        && parent.as_deref() == Some("shadercache")
        && grand.as_deref() == Some("steamapps")
}

fn has_marker(tree: &Tree, node: u32) -> bool {
    tree.children(node).any(|k| {
        PROJECT_MARKERS
            .iter()
            .any(|m| matches_marker(m, tree.name(k)))
    })
}

/// The nearest folder at or above the scanned folder that is a project root,
/// looked up on disk (stops at the home folder and other well-known places).
fn enclosing_project(root: &Path, stop: &[PathBuf]) -> Option<PathBuf> {
    let same = |a: &Path, b: &Path| {
        let k = |p: &Path| {
            p.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_lowercase()
        };
        if cfg!(windows) {
            k(a) == k(b)
        } else {
            a == b
        }
    };
    for (depth, dir) in root.ancestors().skip(1).enumerate() {
        if depth > 12
            || dir.parent().is_none()
            || stop.iter().any(|s| same(s, dir) || path_contains(dir, s))
        {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return None;
        };
        let marked = rd.flatten().any(|e| {
            PROJECT_MARKERS
                .iter()
                .any(|m| matches_marker(m, &e.file_name().to_string_lossy()))
        });
        if marked {
            return Some(dir.to_path_buf());
        }
    }
    None
}

fn find_projects(b: &Builder, node: u32, out: &mut Vec<u32>, not_roots: &HashSet<u32>) {
    let tree = b.tree;
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        for c in b.child_dirs(n) {
            if b.claims.contains_key(&c) {
                continue;
            }
            let name = lower(tree.name(c));
            if NOT_PROJECT_DIRS.contains(&name.as_str()) || name.starts_with('$') {
                continue;
            }
            let is_project = tree.children(c).any(|k| {
                PROJECT_MARKERS
                    .iter()
                    .any(|m| matches_marker(m, tree.name(k)))
            });
            if is_project && tree.nodes[c as usize].size > 0 && !not_roots.contains(&c) {
                out.push(c);
            } else {
                stack.push(c);
            }
        }
    }
}

fn project_kinds(tree: &Tree, p: u32) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    for c in tree.children(p) {
        let n = lower(tree.name(c));
        let k = match n.as_str() {
            "package.json" => "Node.js",
            "cargo.toml" => "Rust",
            "pyproject.toml" | "requirements.txt" => "Python",
            "go.mod" => "Go",
            "pom.xml" | "build.gradle" | "build.gradle.kts" => "Java/Kotlin",
            "composer.json" => "PHP",
            "gemfile" => "Ruby",
            "pubspec.yaml" => "Flutter",
            "project.godot" => "Godot",
            _ if n.ends_with(".sln") || n.ends_with(".csproj") => ".NET",
            _ if n.ends_with(".uproject") => "Unreal",
            _ if n.ends_with(".xcodeproj") => "Xcode",
            ".git" => "Git",
            _ => continue,
        };
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    if kinds.len() > 1 {
        kinds.retain(|k| *k != "Git");
    }
    if kinds.is_empty() {
        "Project".into()
    } else {
        kinds.join(" · ")
    }
}

#[derive(Clone, Copy)]
enum Owner {
    None,
    Entity(u32, u32), // entity, location index
    Anchor(u32, Category),
}

fn attribute_walk(b: &mut Builder, env: &Env, is_volume: bool) -> Storage {
    let tree = b.tree;
    // Folders whose children are worth naming individually in "Other"/"Apps".
    let mut anchor_parents: HashMap<u32, Category> = HashMap::new();
    anchor_parents.insert(0, Category::Other);
    let mut set_anchor = |p: &Option<PathBuf>, cat: Category, b: &Builder| {
        if let Some(n) = p.as_ref().and_then(|p| b.node(p)) {
            anchor_parents.insert(n, cat);
        }
    };
    set_anchor(&env.home, Category::Other, b);
    if let Some(h) = &env.home {
        set_anchor(&h.parent().map(Path::to_path_buf), Category::Other, b);
    }
    for r in env.app_data_roots.clone() {
        set_anchor(&Some(r), Category::Other, b);
    }
    for r in env.program_files.clone() {
        set_anchor(&Some(r), Category::Apps, b);
    }

    let mut anchors: HashMap<u32, u32> = HashMap::new(); // anchor node -> entity
    let mut typed: HashMap<&'static str, u32> = HashMap::new();
    let mut typed_dirs: HashMap<(u32, u32), u64> = HashMap::new(); // (entity, parent dir) -> bytes
    let mut seen_cache_dir: HashSet<u32> = HashSet::new();
    // Hard links: a file's data is only freed when every one of its names
    // goes. Count, per identity, how many live names exist in total.
    let mut link_groups: HashMap<u64, (u32, u64)> = HashMap::new();
    for (&node, &key) in &tree.link_keys {
        let nd = &tree.nodes[node as usize];
        if !nd.is_deleted() {
            let g = link_groups.entry(key).or_default();
            g.0 += 1;
            g.1 = g.1.max(nd.size);
        }
    }
    let mut inside: HashMap<(u32, u8, u64), u32> = HashMap::new();

    // (node, owner, bucket, in_project)
    let mut stack: Vec<(u32, Owner, u8)> = vec![(0, Owner::None, B_FILES)];
    while let Some((n, parent_owner, parent_bucket)) = stack.pop() {
        let node = tree.nodes[n as usize];
        if node.is_deleted() {
            continue;
        }
        let mut owner = parent_owner;
        let mut bucket = parent_bucket;
        if let Some(&(e, bk, loc)) = b.claims.get(&n) {
            owner = Owner::Entity(e, loc);
            bucket = bk;
        } else if node.parent != NONE && !matches!(owner, Owner::Entity(..)) {
            if let Some(&cat) = anchor_parents.get(&node.parent) {
                if node.kind == KIND_DIR {
                    owner = Owner::Anchor(n, cat);
                }
            }
        }
        if node.kind == KIND_DIR || node.first_child != NONE {
            if let Owner::Entity(e, loc) = owner {
                let ent_kind = b.entities[e as usize].kind;
                let name = lower(tree.name(n));
                let in_data = b.entities[e as usize].locations[loc as usize].bucket == B_DATA;
                if matches!(ent_kind, EntityKind::App | EntityKind::Game)
                    && in_data
                    && bucket == B_DATA
                    && !b.claims.contains_key(&n)
                    && CACHE_DIR_NAMES.contains(&name.as_str())
                {
                    bucket = B_CACHE;
                    if seen_cache_dir.insert(n) {
                        b.entities[e as usize].cache_dirs.push(n);
                    }
                } else if ent_kind == EntityKind::Project && bucket != B_BUILD {
                    let parent = node.parent;
                    let junk = JUNK_DIRS.iter().any(|(d, markers)| {
                        name == *d
                            && (markers.contains(&"*")
                                || tree.children(parent).any(|s| {
                                    markers.iter().any(|m| matches_marker(m, tree.name(s)))
                                }))
                    });
                    if junk {
                        bucket = B_BUILD;
                        b.entities[e as usize].junk_dirs.push(n);
                    }
                }
            }
            if node.first_child != NONE {
                for c in tree.children(n) {
                    stack.push((c, owner, bucket));
                }
            }
            continue;
        }
        // Files and pseudo items: attribute their bytes.
        let size = node.size;
        let is_file = node.kind == KIND_FILE;
        if node.kind == KIND_LINK {
            continue;
        }
        let target: u32 = match owner {
            Owner::Entity(e, loc) => {
                b.entities[e as usize].locations[loc as usize].bytes += size;
                e
            }
            _ => {
                let cat = if is_file {
                    category_of(tree.name(n))
                } else {
                    "Other"
                };
                let typed_target = match cat {
                    "Images" => Some(("Photos & images elsewhere", Category::Media)),
                    "Video" => Some(("Videos elsewhere", Category::Media)),
                    "Audio" => Some(("Audio elsewhere", Category::Media)),
                    "Documents" => Some(("Documents elsewhere", Category::Documents)),
                    _ => None,
                };
                if let (Some((name, tc)), true) = (typed_target, size > 0) {
                    let e = *typed.entry(name).or_insert_with(|| {
                        let id = b.entities.len() as u32;
                        let mut ent = Entity::new(id, tc, EntityKind::FileType, name);
                        ent.detail = Some(
                            "Files of this type in folders that don't belong to anything else"
                                .into(),
                        );
                        b.entities.push(ent);
                        id
                    });
                    *typed_dirs.entry((e, node.parent)).or_default() += size;
                    e
                } else {
                    match owner {
                        Owner::Anchor(a, cat) => *anchors.entry(a).or_insert_with(|| {
                            let id = b.entities.len() as u32;
                            let aname = tree.name(a);
                            let lname = aname.to_lowercase();
                            let cat = if cat != Category::Other && cat != Category::Apps {
                                cat
                            } else if crate::catalog::is_game_publisher(aname)
                                || lname == "games"
                                || lname.ends_with(" games")
                            {
                                Category::Games
                            } else {
                                cat
                            };
                            let mut ent =
                                Entity::new(id, cat, EntityKind::Folder, aname.to_string());
                            ent.detail = Some(anchor_detail(tree, a, cat, env));
                            if matches!(cat, Category::Apps | Category::Games) {
                                ent.icons = vec![IconHint::ExeIn { dir: tree.path(a) }];
                            }
                            ent.locations.push(Location {
                                node: a,
                                bucket: B_FILES,
                                bytes: 0,
                            });
                            b.entities.push(ent);
                            id
                        }),
                        _ => *anchors.entry(node.parent).or_insert_with(|| {
                            let id = b.entities.len() as u32;
                            let name = if is_volume {
                                "Files at the top of the drive"
                            } else {
                                "Loose files"
                            };
                            let mut ent =
                                Entity::new(id, Category::Other, EntityKind::Folder, name);
                            ent.locations.push(Location {
                                node: node.parent,
                                bucket: B_FILES,
                                bytes: 0,
                            });
                            b.entities.push(ent);
                            id
                        }),
                    }
                }
            }
        };
        let ent = &mut b.entities[target as usize];
        if let Owner::Anchor(..) | Owner::None = owner {
            if ent.kind == EntityKind::Folder {
                if let Some(l) = ent.locations.first_mut() {
                    l.bytes += size;
                }
            }
        }
        ent.size += size;
        ent.buckets[bucket.min(3) as usize] += size;
        if is_file {
            ent.files += 1;
        }
        if matches!(owner, Owner::Entity(..)) && (bucket == B_CACHE || bucket == B_BUILD) {
            match tree.link_keys.get(&n) {
                Some(&k) => *inside.entry((target, bucket, k)).or_default() += 1,
                None if bucket == B_CACHE => ent.reclaim_cache += size,
                None => ent.reclaim_build += size,
            }
        }
    }
    for ((e, bucket, k), count) in inside {
        if let Some(&(total, size)) = link_groups.get(&k) {
            if count == total {
                let ent = &mut b.entities[e as usize];
                if bucket == B_CACHE {
                    ent.reclaim_cache += size;
                } else {
                    ent.reclaim_build += size;
                }
            }
        }
    }

    // Typed entities: their locations are the folders holding most of them.
    let mut by_entity: HashMap<u32, Vec<(u32, u64)>> = HashMap::new();
    for ((e, dir), bytes) in typed_dirs {
        by_entity.entry(e).or_default().push((dir, bytes));
    }
    for (e, mut dirs) in by_entity {
        dirs.sort_by_key(|d| std::cmp::Reverse(d.1));
        dirs.truncate(40);
        b.entities[e as usize].locations = dirs
            .into_iter()
            .map(|(node, bytes)| Location {
                node,
                bucket: B_FILES,
                bytes,
            })
            .collect();
    }

    let total: u64 = tree.nodes[0].size;
    let mut entities: Vec<Entity> = std::mem::take(&mut b.entities);
    for e in &mut entities {
        e.locations.retain(|l| l.bytes > 0);
        e.locations.sort_by_key(|l| std::cmp::Reverse(l.bytes));
    }
    let categories = ALL_CATEGORIES
        .iter()
        .map(|&c| CategoryTotal {
            category: c,
            size: entities
                .iter()
                .filter(|e| e.category == c)
                .map(|e| e.size)
                .sum(),
            count: entities
                .iter()
                .filter(|e| e.category == c && e.size > 0)
                .count() as u32,
        })
        .collect();
    Storage {
        entities,
        categories,
        total,
        generation: tree.generation,
    }
}

fn anchor_detail(tree: &Tree, a: u32, cat: Category, env: &Env) -> String {
    let parent = tree.nodes[a as usize].parent;
    let pname = if parent == NONE {
        String::new()
    } else {
        tree.name(parent).to_string()
    };
    if cat == Category::Apps {
        return format!("In {pname} · not in the system's app list");
    }
    let path = tree.path(a);
    let is_data_root = env
        .app_data_roots
        .iter()
        .any(|r| path.parent().map(|p| p == r.as_path()).unwrap_or(false));
    if is_data_root {
        format!("App data in {pname}")
    } else if env
        .home
        .as_ref()
        .map(|h| path.parent() == Some(h.as_path()))
        .unwrap_or(false)
    {
        "In your user folder".into()
    } else if parent == 0 {
        "Folder at the top of the scan".into()
    } else {
        format!("In {pname}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::AppRecord;

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

    fn d(name: &str, files: Vec<RawEntry>, dirs: Vec<RawDir>) -> RawDir {
        let mut r = RawDir::new(name.into(), 0);
        r.files = files;
        r.dirs = dirs;
        r.finish();
        r
    }

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" })
    }

    /// A small "drive" with an app installed under Program Files, its data in
    /// AppData (including a cache), a game, a project inside Documents, user
    /// media, Windows, a page file, unknown stuff and an unreadable gap.
    fn fixture() -> (Tree, Catalog, Env) {
        let appdata_local = d(
            "Local",
            vec![],
            vec![
                d(
                    "Acme",
                    vec![],
                    vec![d(
                        "Paint",
                        vec![f("settings.db", 300)],
                        vec![d("Cache", vec![f("c1", 700)], vec![])],
                    )],
                ),
                d("Temp", vec![f("junk.tmp", 50)], vec![]),
                d("Mystery", vec![f("blob.bin", 11)], vec![]),
            ],
        );
        let profile = d(
            "ann",
            vec![f("notes.bin", 5), f("package.json", 1)],
            vec![
                d(
                    "AppData",
                    vec![],
                    vec![
                        appdata_local,
                        d(
                            "Roaming",
                            vec![],
                            vec![d("Paint", vec![f("prefs.json", 40)], vec![])],
                        ),
                    ],
                ),
                d(
                    "Documents",
                    vec![f("cv.pdf", 20)],
                    vec![d(
                        "webapp",
                        vec![f("package.json", 1), f("index.ts", 2)],
                        vec![d(
                            "node_modules",
                            vec![f("dep.js", 900)],
                            vec![d("nested", vec![f("package.json", 1)], vec![])],
                        )],
                    )],
                ),
                d(
                    "Downloads",
                    vec![f("setup.exe", 400), f("movie.mp4", 1000)],
                    vec![],
                ),
                d("stuff", vec![f("photo.jpg", 60), f("data.xyz", 8)], vec![]),
            ],
        );
        let users = d("Users", vec![], vec![profile]);
        let pf = d(
            "Program Files",
            vec![],
            vec![
                d(
                    "Acme Paint",
                    vec![f("paint.exe", 2000)],
                    vec![d("plugins", vec![f("p.dll", 100)], vec![])],
                ),
                d("Unregistered Tool", vec![f("tool.exe", 77)], vec![]),
            ],
        );
        let windows = d(
            "Windows",
            vec![f("explorer.exe", 3000)],
            vec![d("WinSxS", vec![f("x.dll", 4000)], vec![])],
        );
        let games = d(
            "Games",
            vec![],
            vec![d("Space Game", vec![f("game.pak", 9000)], vec![])],
        );
        let mut gap = RawDir::new("System & unreadable space".into(), 0);
        gap.kind = KIND_GAP;
        gap.files.push(RawEntry {
            name: "Restore points".into(),
            size: 123,
            logical: 123,
            mtime: 0,
            kind: KIND_GAP_PART,
            flags: 0,
            key: 0,
        });
        gap.finish();
        gap.total_files = 0;
        let mut drive = d(
            "C",
            vec![f("pagefile.sys", 5000)],
            vec![users, pf, windows, games],
        );
        drive.dirs.push(gap);
        drive.finish();
        let r = root();
        let tree = Tree::from_raw(drive, r.to_string_lossy().into_owned());

        let home = r.join("Users").join("ann");
        let local = home.join("AppData").join("Local");
        let env = Env {
            home: Some(home.clone()),
            app_data_roots: vec![local.clone(), home.join("AppData").join("Roaming")],
            program_files: vec![r.join("Program Files")],
            system_root: Some(r.join("Windows")),
            system_drive: Some(r.clone()),
            downloads: Some(home.join("Downloads")),
            documents: Some(home.join("Documents")),
            desktop: None,
            pictures: None,
            videos: None,
            music: None,
            onedrive: None,
            temp: Some(local.join("Temp")),
        };
        let mut paint = AppRecord::new("Acme Paint 2.1 (x64)", AppKind::App, "Windows");
        paint.publisher = Some("Acme Inc.".into());
        paint
            .install_dirs
            .push(r.join("Program Files").join("Acme Paint"));
        paint.aliases.push("paint".into());
        paint.uninstall = Some(Uninstall::Command {
            command: "uninst.exe".into(),
        });
        paint.icons.push(IconHint::File {
            path: r.join("Program Files").join("Acme Paint").join("paint.exe"),
            index: 0,
        });
        let mut game = AppRecord::new("Space Game", AppKind::Game, "Steam");
        game.install_dirs.push(r.join("Games").join("Space Game"));
        // A bogus registry entry pointing at Program Files itself must be ignored.
        let mut bogus = AppRecord::new("Broken Entry", AppKind::App, "Windows");
        bogus.install_dirs.push(r.join("Program Files"));
        (
            tree,
            Catalog {
                apps: vec![paint, game, bogus],
            },
            env,
        )
    }

    fn by_name<'a>(s: &'a Storage, name: &str) -> &'a Entity {
        s.entities
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no entity {name}"))
    }

    #[test]
    fn apps_carry_their_icon_hints() {
        let (tree, cat, env) = fixture();
        let s = attribute(&tree, &cat, &env, true);
        let paint = by_name(&s, "Acme Paint 2.1 (x64)");
        assert_eq!(paint.icons, cat.apps[0].icons);
        // Caches, file types and system items keep the letter tile.
        assert!(s
            .entities
            .iter()
            .filter(|e| !matches!(
                e.kind,
                EntityKind::App | EntityKind::Game | EntityKind::Folder
            ))
            .all(|e| e.icons.is_empty()));
    }

    #[test]
    fn every_byte_counted_exactly_once() {
        let (tree, cat, env) = fixture();
        let s = attribute(&tree, &cat, &env, true);
        let sum: u64 = s.entities.iter().map(|e| e.size).sum();
        assert_eq!(
            sum, tree.nodes[0].size,
            "entities must add up to the scan total"
        );
        let cats: u64 = s.categories.iter().map(|c| c.size).sum();
        assert_eq!(cats, s.total);
        for e in &s.entities {
            assert_eq!(
                e.buckets.iter().sum::<u64>(),
                e.size,
                "{} buckets add up",
                e.name
            );
        }
    }

    #[test]
    fn app_owns_install_data_and_cache() {
        let (tree, cat, env) = fixture();
        let s = attribute(&tree, &cat, &env, true);
        let paint = by_name(&s, "Acme Paint 2.1 (x64)");
        assert_eq!(paint.category, Category::Apps);
        assert_eq!(paint.buckets[B_FILES as usize], 2100);
        // Local\Acme\Paint (publisher folder) + Roaming\Paint
        assert_eq!(paint.buckets[B_DATA as usize], 340);
        assert_eq!(paint.buckets[B_CACHE as usize], 700);
        assert_eq!(paint.cache_dirs.len(), 1);
        assert!(matches!(paint.uninstall, Some(Uninstall::Command { .. })));
    }

    #[test]
    fn categories_are_assigned() {
        let (tree, cat, env) = fixture();
        let s = attribute(&tree, &cat, &env, true);
        assert_eq!(by_name(&s, "Space Game").size, 9000);
        assert_eq!(by_name(&s, "Space Game").category, Category::Games);
        let proj = by_name(&s, "webapp");
        assert_eq!(proj.category, Category::Projects);
        assert_eq!(
            proj.buckets[B_BUILD as usize], 901,
            "node_modules is reclaimable build/deps"
        );
        assert_eq!(proj.size, 904);
        assert_eq!(
            by_name(&s, "Documents").size,
            20,
            "the project inside Documents isn't double counted"
        );
        assert_eq!(by_name(&s, "Downloads").size, 1400);
        assert_eq!(by_name(&s, "Windows").size, 3000);
        assert_eq!(by_name(&s, "Windows component store (WinSxS)").size, 4000);
        assert_eq!(by_name(&s, "Page file").size, 5000);
        assert_eq!(
            by_name(&s, "Restore points, reserved & unreadable").size,
            123
        );
        assert_eq!(by_name(&s, "Temporary files").size, 50);
        assert_eq!(by_name(&s, "Temporary files").category, Category::Caches);
        assert_eq!(by_name(&s, "Photos & images elsewhere").size, 60);
        assert!(
            s.entities
                .iter()
                .all(|e| !(e.name == "ann" && e.category == Category::Projects)),
            "the home folder is never a project"
        );
        let tool = by_name(&s, "Unregistered Tool");
        assert_eq!((tool.category, tool.size), (Category::Apps, 77));
        assert_eq!(by_name(&s, "Mystery").category, Category::Other);
        assert_eq!(by_name(&s, "stuff").size, 8);
        assert!(s
            .entities
            .iter()
            .all(|e| e.name != "Broken Entry" || e.size == 0));
    }

    #[test]
    fn deletions_keep_reconciling() {
        let (mut tree, cat, env) = fixture();
        let dl = find_by_path(&tree, &env.downloads.clone().unwrap()).unwrap();
        tree.remove(tree.child_by_name(dl, "movie.mp4").unwrap());
        let s = attribute(&tree, &cat, &env, true);
        assert_eq!(
            s.entities.iter().map(|e| e.size).sum::<u64>(),
            tree.nodes[0].size
        );
        assert_eq!(by_name(&s, "Downloads").size, 400);
    }

    #[test]
    fn folder_scans_are_attributed_to_their_enclosing_location() {
        let (_, cat, env) = fixture();
        // Scanning a folder inside Downloads: all of it is Downloads.
        let sub = d("sub", vec![f("a.zip", 10), f("b.mp4", 20)], vec![]);
        let root = env.downloads.clone().unwrap().join("sub");
        let tree = Tree::from_raw(sub, root.to_string_lossy().into_owned());
        let s = attribute(&tree, &cat, &env, false);
        assert_eq!(by_name(&s, "Downloads").size, 30);
        assert_eq!(s.entities.iter().map(|e| e.size).sum::<u64>(), 30);

        // Scanning a project folder directly: the root is the project.
        let proj = d(
            "game",
            vec![f("Cargo.toml", 1)],
            vec![
                d("target", vec![f("big.rlib", 500)], vec![]),
                d("src", vec![f("main.rs", 9)], vec![]),
            ],
        );
        let root = env.documents.clone().unwrap().join("game");
        let tree = Tree::from_raw(proj, root.to_string_lossy().into_owned());
        let s = attribute(&tree, &cat, &env, false);
        let p = by_name(&s, "game");
        assert_eq!((p.category, p.size), (Category::Projects, 510));
        assert_eq!(p.reclaim_build, 500);
    }

    #[test]
    fn only_regenerable_folders_can_be_cleaned() {
        let (_, cat, env) = fixture();
        let r = root();
        // Unreal: Saved (autosaves, save games) is never build output.
        let ue = d(
            "Shooter",
            vec![f("Shooter.uproject", 1)],
            vec![
                d("Saved", vec![f("autosave.umap", 70)], vec![]),
                d("Intermediate", vec![f("x.obj", 30)], vec![]),
            ],
        );
        // An app install folder with folders named like caches.
        let app = d(
            "Acme Paint",
            vec![f("paint.exe", 10)],
            vec![
                d("Cache", vec![f("bundled.dat", 40)], vec![]),
                d("Logs", vec![f("l.txt", 3)], vec![]),
            ],
        );
        let pf = d("Program Files", vec![], vec![app]);
        let docs = d("Documents", vec![], vec![ue]);
        let home = d("ann", vec![], vec![docs]);
        let users = d("Users", vec![], vec![home]);
        let mut drive = d("C", vec![], vec![users, pf]);
        drive.finish();
        let tree = Tree::from_raw(drive, r.to_string_lossy().into_owned());
        let s = attribute(&tree, &cat, &env, true);
        let shooter = by_name(&s, "Shooter");
        assert_eq!(
            shooter.reclaim_build, 30,
            "only Intermediate is rebuildable"
        );
        assert_eq!(shooter.junk_dirs.len(), 1);
        let paint = by_name(&s, "Acme Paint 2.1 (x64)");
        assert!(
            paint.cache_dirs.is_empty(),
            "install folders are never cleared"
        );
        assert_eq!(paint.buckets[B_CACHE as usize], 0);
        assert_eq!(paint.reclaim_cache, 0);
    }

    #[test]
    fn steam_shader_cache_is_clearable_and_validated() {
        let r = root();
        let lib = d(
            "steamapps",
            vec![],
            vec![
                d(
                    "common",
                    vec![],
                    vec![d("Dota", vec![f("game.vpk", 900)], vec![])],
                ),
                d(
                    "shadercache",
                    vec![],
                    vec![
                        d("570", vec![f("s.bin", 60)], vec![]),
                        d("evil", vec![f("e", 5)], vec![]),
                    ],
                ),
            ],
        );
        let mut drive = d("C", vec![], vec![d("Steam", vec![], vec![lib])]);
        drive.finish();
        let tree = Tree::from_raw(drive, r.to_string_lossy().into_owned());
        let sa = r.join("Steam").join("steamapps");
        let mut game = AppRecord::new("Dota 2", AppKind::Game, "Steam");
        game.install_dirs.push(sa.join("common").join("Dota"));
        game.cache_dirs.push(sa.join("shadercache").join("570"));
        game.cache_dirs.push(sa.join("shadercache").join("evil"));
        let s = attribute(&tree, &Catalog { apps: vec![game] }, &Env::default(), true);
        let g = by_name(&s, "Dota 2");
        assert_eq!(g.cache_dirs.len(), 1, "only the numeric app id folder");
        assert_eq!(g.reclaim_cache, 60);
        assert_eq!(
            s.entities.iter().map(|e| e.size).sum::<u64>(),
            tree.nodes[0].size
        );
    }

    #[test]
    fn reclaimable_build_respects_hard_links_outside_the_project() {
        let r = root();
        let mut nm_file = f("dep.js", 100);
        nm_file.key = 9;
        let mut store_file = f("dep.js", 0);
        store_file.key = 9;
        store_file.flags = FLAG_HARDLINK;
        let proj = d(
            "app",
            vec![f("package.json", 1)],
            vec![d("node_modules", vec![nm_file, f("own.js", 7)], vec![])],
        );
        let store = d("store", vec![store_file], vec![]);
        let mut drive = d("C", vec![], vec![proj, store]);
        drive.finish();
        let tree = Tree::from_raw_with_links(
            drive,
            r.to_string_lossy().into_owned(),
            [9u64].into_iter().collect(),
        );
        let s = attribute(&tree, &Catalog::default(), &Env::default(), false);
        let p = by_name(&s, "app");
        assert_eq!(
            p.buckets[B_BUILD as usize], 107,
            "attribution: the counted name is inside"
        );
        assert_eq!(
            p.reclaim_build, 7,
            "deleting node_modules frees only what has no other name"
        );
    }

    #[test]
    fn every_part_of_the_gap_gets_its_bytes() {
        let r = root();
        let mut gap = RawDir::new("System & unreadable space".into(), 0);
        gap.kind = KIND_GAP;
        for (name, size) in [
            ("NTFS file table ($MFT)", 300),
            ("Recycle Bin (not listable)", 200),
            ("Restore points, reserved & unreadable", 500),
        ] {
            gap.files.push(RawEntry {
                name: name.into(),
                size,
                logical: size,
                kind: KIND_GAP_PART,
                ..Default::default()
            });
        }
        gap.finish();
        gap.total_files = 0;
        let mut drive = d("C", vec![f("a.bin", 10)], vec![]);
        drive.dirs.push(gap);
        drive.finish();
        let tree = Tree::from_raw(drive, r.to_string_lossy().into_owned());
        let s = attribute(&tree, &Catalog::default(), &Env::default(), true);
        assert_eq!(by_name(&s, "NTFS file table ($MFT)").size, 300);
        assert_eq!(by_name(&s, "Recycle Bin").size, 200);
        assert_eq!(by_name(&s, "Recycle Bin").category, Category::Caches);
        assert_eq!(
            by_name(&s, "Restore points, reserved & unreadable").size,
            500
        );
        assert_eq!(s.entities.iter().map(|e| e.size).sum::<u64>(), 1010);
    }

    #[test]
    fn installed_apps_are_never_projects() {
        let (_, _, env) = fixture();
        let r = root();
        let app_dir = r.join("Program Files").join("Chatty");
        let mut chatty = AppRecord::new("Chatty", AppKind::App, "Windows");
        chatty.install_dirs.push(app_dir.clone());
        let cat = Catalog { apps: vec![chatty] };
        let resources = || {
            d(
                "resources",
                vec![],
                vec![d(
                    "app",
                    vec![f("package.json", 1)],
                    vec![d("node_modules", vec![f("dep.js", 900)], vec![])],
                )],
            )
        };
        // Whole-drive scan: the installed app owns it all; no project, no junk.
        let pf = d(
            "Program Files",
            vec![],
            vec![d("Chatty", vec![f("chatty.exe", 50)], vec![resources()])],
        );
        let mut drive = d("C", vec![], vec![pf]);
        drive.finish();
        let tree = Tree::from_raw(drive, r.to_string_lossy().into_owned());
        let s = attribute(&tree, &cat, &env, true);
        assert!(s
            .entities
            .iter()
            .all(|e| e.category != Category::Projects || e.size == 0));
        assert!(crate::suggest::project_junk_from(&s.entities, &tree).is_none());
        // Scanning the app's resources\app folder directly: still the app's.
        let root_dir = app_dir.join("resources").join("app");
        let direct = d(
            "app",
            vec![f("package.json", 1)],
            vec![d("node_modules", vec![f("dep.js", 900)], vec![])],
        );
        let tree = Tree::from_raw(direct, root_dir.to_string_lossy().into_owned());
        let s = attribute(&tree, &cat, &env, false);
        assert_eq!(by_name(&s, "Chatty").size, 901);
        assert!(s.entities.iter().all(|e| e.category != Category::Projects));
        assert!(s.entities.iter().all(|e| e.junk_dirs.is_empty()));
    }
}
