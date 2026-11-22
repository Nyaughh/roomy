//! Roomy: Tauri commands and app state.

pub mod catalog;
pub mod delete;
pub mod dupes;
pub mod icons;
pub mod safety;
pub mod scan;
pub mod storage;
pub mod suggest;
pub mod system;
pub mod tree;
pub mod views;

use delete::Mode;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tree::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GapPart {
    pub label: String,
    pub bytes: Option<u64>,
    pub explain: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub root_path: String,
    pub is_volume: bool,
    pub volume_total: Option<u64>,
    pub volume_free: Option<u64>,
    pub used: Option<u64>,
    /// Bytes attributed to real files (excludes the gap).
    pub scanned_bytes: u64,
    pub scanned_logical: u64,
    pub files: u64,
    pub dirs: u64,
    pub denied_dirs: u64,
    pub denied_samples: Vec<String>,
    /// used - scanned. Negative when compression/dedup make files look bigger.
    pub gap: i64,
    pub gap_parts: Vec<GapPart>,
    pub hardlink_bytes: u64,
    pub links_skipped: u64,
    pub elapsed_ms: u64,
    pub elevated: bool,
    pub generation: u64,
}

struct ScanData {
    tree: Tree,
    summary: Summary,
    catalog: Arc<catalog::Catalog>,
    /// Storage attribution, cached for the tree generation it was built from.
    storage: Option<Arc<storage::Storage>>,
    /// NTFS master file table size, when Windows reports it.
    mft: Option<u64>,
}

struct AppState {
    data: RwLock<Option<ScanData>>,
    scanning: AtomicBool,
    cancel: Mutex<Arc<AtomicBool>>,
    suggestions: Mutex<Vec<suggest::Suggestion>>,
    dupes_cancel: Mutex<Arc<AtomicBool>>,
    guard: safety::Guard,
    pool: Arc<rayon::ThreadPool>,
}

type St<'a> = State<'a, AppState>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    files: u64,
    dirs: u64,
    bytes: u64,
    denied: u64,
    current: String,
    elapsed_ms: u64,
    /// Expected total (used space) for whole-drive scans.
    target: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanDone {
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    summary: Option<Summary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    elevated: bool,
    platform: &'static str,
    version: &'static str,
    home: Option<String>,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        elevated: system::is_elevated(),
        platform: if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        },
        version: env!("CARGO_PKG_VERSION"),
        home: dirs::home_dir().map(|p| p.to_string_lossy().into_owned()),
    }
}

#[tauri::command]
async fn list_volumes() -> Vec<system::Volume> {
    tauri::async_runtime::spawn_blocking(system::list_volumes)
        .await
        .unwrap_or_default()
}

#[tauri::command]
fn relaunch_elevated(app: AppHandle) -> Result<(), String> {
    system::relaunch_elevated()?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn open_storage_settings() -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:storagesense")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("Only available on Windows.".into())
    }
}

pub const GAP_MFT: &str = "NTFS file table ($MFT)";
pub const GAP_BIN: &str = "Recycle Bin (not listable)";
pub const GAP_REST: &str = "Restore points, reserved & unreadable";

/// The pseudo folder that holds used space no folder listing shows. It's
/// created for every drive scan (even when empty) so later changes can
/// re-balance it against the drive's real used space.
fn gap_dir(has_mft: bool) -> RawDir {
    let mut dir = RawDir::new("System & unreadable space".into(), 0);
    dir.kind = KIND_GAP;
    let mut names = Vec::new();
    if has_mft {
        names.push(GAP_MFT);
    }
    if cfg!(windows) {
        names.push(GAP_BIN);
    }
    names.push(GAP_REST);
    for name in names {
        dir.files.push(RawEntry {
            name: name.into(),
            kind: KIND_GAP_PART,
            ..Default::default()
        });
    }
    dir.finish();
    dir.total_files = 0;
    dir
}

/// Split the gap into its parts: the MFT and the unlistable part of the
/// Recycle Bin when known, the rest unexplained.
pub fn split_gap(gap: u64, mft: Option<u64>, bin_unlisted: u64) -> (u64, u64, u64) {
    let mft = mft.unwrap_or(0).min(gap);
    let bin = bin_unlisted.min(gap - mft);
    (mft, bin, gap - mft - bin)
}

fn gap_explain(name: &str, denied: u64, elevated: bool) -> String {
    match name {
        GAP_MFT => "The index of every file on the drive. It grows with the number of files and Windows manages it.".into(),
        GAP_BIN => "Files in the Recycle Bin that Windows doesn't let Roomy list one by one. Empty the bin to free this.".into(),
        _ if cfg!(windows) => {
            let mut s = String::from(
                "Space Windows reports as used that no folder listing shows: System Restore points and shadow copies (in System Volume Information), file system metadata and reserved storage for updates",
            );
            if denied > 0 && !elevated {
                s.push_str(&format!(", plus {denied} folders Roomy wasn't allowed to open"));
            }
            s.push_str(". Run Roomy as administrator to look inside more of it.");
            s
        }
        _ => format!(
            "Space the system reports as used that Roomy couldn't attribute to a file: snapshots, file system metadata, purgeable space{}.",
            if denied > 0 {
                format!(" and {denied} folders Roomy wasn't allowed to open")
            } else {
                String::new()
            }
        ),
    }
}

/// Re-read the drive's used space and re-balance the gap node so the tree
/// total always equals what the OS reports as used. Call after any change.
fn reconcile_gap(d: &mut ScanData) {
    if !d.summary.is_volume {
        return;
    }
    let root = PathBuf::from(&d.summary.root_path);
    let Some(u) = system::disk_usage(&root) else {
        return;
    };
    let used = u.total.saturating_sub(u.free);
    let Some(gap_node) = d
        .tree
        .children(0)
        .find(|&c| d.tree.nodes[c as usize].kind == KIND_GAP)
    else {
        return;
    };
    let scanned = d.tree.nodes[0]
        .size
        .saturating_sub(d.tree.nodes[gap_node as usize].size);
    let gap = used as i64 - scanned as i64;
    // Recycle Bin bytes Windows reports but the scan couldn't list.
    let bin_listed = d
        .tree
        .children(0)
        .find(|&c| d.tree.name(c).eq_ignore_ascii_case("$Recycle.Bin"))
        .map(|c| d.tree.nodes[c as usize].size)
        .unwrap_or(0);
    let bin_total = system::recycle_bin_size_on(&root)
        .map(|(b, _)| b)
        .unwrap_or(0);
    let (mft, bin, rest) = split_gap(
        gap.max(0) as u64,
        d.mft,
        bin_total.saturating_sub(bin_listed),
    );
    let parts: Vec<u32> = d.tree.children(gap_node).collect();
    let mut gap_parts = Vec::new();
    for p in parts {
        let name = d.tree.name(p).to_string();
        let bytes = match name.as_str() {
            GAP_MFT => mft,
            GAP_BIN => bin,
            _ => rest,
        };
        d.tree.set_leaf_size(p, bytes, bytes);
        if bytes > 0 {
            gap_parts.push(GapPart {
                explain: gap_explain(&name, d.summary.denied_dirs, d.summary.elevated),
                label: name,
                bytes: Some(bytes),
            });
        }
    }
    d.summary.used = Some(used);
    d.summary.volume_free = Some(u.free);
    d.summary.volume_total = Some(u.total);
    d.summary.gap = gap;
    d.summary.gap_parts = gap_parts;
}

#[tauri::command]
fn start_scan(app: AppHandle, state: St, path: String) -> Result<(), String> {
    let raw = path.trim();
    let raw = if cfg!(windows) {
        raw.replace('/', "\\")
    } else {
        raw.to_string()
    };
    let root = PathBuf::from(raw);
    let md = std::fs::metadata(&root).map_err(|e| format!("Can't open {}: {e}", root.display()))?;
    if !md.is_dir() {
        return Err("Pick a folder or a drive to scan.".into());
    }
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("A scan is already running.".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *state.cancel.lock() = cancel.clone();
    *state.suggestions.lock() = Vec::new();
    let pool = state.pool.clone();
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("roomy-scan".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_scan(&app2, &pool, root, cancel.clone())
            }));
            let st = app2.state::<AppState>();
            let done = match result {
                Ok(Some(data)) => {
                    let summary = data.summary.clone();
                    *st.data.write() = Some(data);
                    ScanDone {
                        ok: true,
                        cancelled: false,
                        error: None,
                        summary: Some(summary),
                    }
                }
                Ok(None) => ScanDone {
                    ok: false,
                    cancelled: true,
                    error: None,
                    summary: None,
                },
                Err(_) => ScanDone {
                    ok: false,
                    cancelled: false,
                    error: Some("The scan stopped unexpectedly. Please try again.".into()),
                    summary: None,
                },
            };
            st.scanning.store(false, Ordering::SeqCst);
            let _ = app2.emit("scan-done", done);
        })
        .map_err(|e| {
            state.scanning.store(false, Ordering::SeqCst);
            e.to_string()
        })?;
    Ok(())
}

fn run_scan(
    app: &AppHandle,
    pool: &rayon::ThreadPool,
    root: PathBuf,
    cancel: Arc<AtomicBool>,
) -> Option<ScanData> {
    let elevated = system::is_elevated();
    if elevated {
        system::enable_backup_privilege();
    }
    let is_volume = system::is_volume_root(&root);
    let usage = if is_volume {
        system::disk_usage(&root)
    } else {
        None
    };
    let used = usage.map(|u| u.total.saturating_sub(u.free));
    let progress = Arc::new(scan::Progress::default());
    let ctx = scan::ScanCtx::new(&root, cancel.clone(), progress.clone());
    let started = Instant::now();

    // Stream progress to the UI while the pool works.
    let finished = Arc::new(AtomicBool::new(false));
    let emitter = {
        let app = app.clone();
        let progress = progress.clone();
        let finished = finished.clone();
        std::thread::spawn(move || {
            while !finished.load(Ordering::Relaxed) {
                let p = ScanProgress {
                    files: progress.files.load(Ordering::Relaxed),
                    dirs: progress.dirs.load(Ordering::Relaxed),
                    bytes: progress.bytes.load(Ordering::Relaxed),
                    denied: progress.denied.load(Ordering::Relaxed),
                    current: progress.current.lock().clone(),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    target: used,
                };
                let _ = app.emit("scan-progress", p);
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let mut raw = pool.install(|| scan::scan_dir(&ctx, &root));
    finished.store(true, Ordering::Relaxed);
    let _ = emitter.join();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let root_name = root.to_string_lossy().into_owned();
    let scanned = raw.total_size;
    let scanned_logical = raw.total_logical;
    let files = raw.total_files;
    let denied = progress.denied.load(Ordering::Relaxed);
    let mft = if is_volume {
        system::ntfs_mft_bytes(&root)
    } else {
        None
    };
    if is_volume {
        raw.dirs.push(gap_dir(mft.is_some()));
        raw.finish();
    }
    let dup_keys = std::mem::take(&mut *ctx.dup_keys.lock());
    let tree = Tree::from_raw_with_links(raw, root_name.clone(), dup_keys);
    let denied_paths = ctx.denied_paths.lock().clone();
    let summary = Summary {
        root_path: root_name,
        is_volume,
        volume_total: usage.map(|u| u.total),
        volume_free: usage.map(|u| u.free),
        used,
        scanned_bytes: scanned,
        scanned_logical,
        files,
        dirs: progress.dirs.load(Ordering::Relaxed),
        denied_dirs: denied,
        denied_samples: denied_paths.into_iter().take(50).collect(),
        gap: 0,
        gap_parts: Vec::new(),
        hardlink_bytes: ctx.hardlink_bytes.load(Ordering::Relaxed),
        links_skipped: ctx.links_skipped.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().as_millis() as u64,
        elevated,
        generation: tree.generation,
    };
    // What's installed (registry, launchers). Cheap: no disk walk.
    let catalog = Arc::new(catalog::load());
    let mut data = ScanData {
        tree,
        summary,
        catalog,
        storage: None,
        mft,
    };
    reconcile_gap(&mut data);
    data.summary.generation = data.tree.generation;
    Some(data)
}

#[tauri::command]
fn cancel_scan(state: St) {
    state.cancel.lock().store(true, Ordering::SeqCst);
}

#[tauri::command]
fn close_scan(state: St) {
    *state.data.write() = None;
    *state.suggestions.lock() = Vec::new();
}

fn with_data<T>(state: &St, f: impl FnOnce(&ScanData) -> Result<T, String>) -> Result<T, String> {
    let g = state.data.read();
    let d = g.as_ref().ok_or("No scan yet.")?;
    f(d)
}

fn live(tree: &Tree, id: u32) -> Result<(), String> {
    tree.get(id)
        .map(|_| ())
        .ok_or_else(|| "That item is no longer there.".into())
}

#[tauri::command]
fn get_summary(state: St) -> Option<Summary> {
    state.data.read().as_ref().map(|d| d.summary.clone())
}

#[tauri::command]
fn get_children(state: St, id: u32, limit: Option<usize>) -> Result<views::Children, String> {
    with_data(&state, |d| {
        live(&d.tree, id)?;
        Ok(views::children(&d.tree, id, limit.unwrap_or(5000)))
    })
}

#[tauri::command]
fn get_treemap(state: St, id: u32, depth: Option<u32>) -> Result<views::TreemapNode, String> {
    with_data(&state, |d| {
        live(&d.tree, id)?;
        Ok(views::treemap(
            &d.tree,
            id,
            depth.unwrap_or(3).min(5),
            0.0004,
        ))
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeDetails {
    node: views::NodeInfo,
    path: String,
    crumbs: Vec<views::Crumb>,
    pseudo: bool,
    blocked: Option<String>,
}

#[tauri::command]
fn get_node(state: St, id: u32) -> Result<NodeDetails, String> {
    with_data(&state, |d| {
        live(&d.tree, id)?;
        let pseudo = d.tree.is_pseudo(id);
        let path = d.tree.path(id);
        let blocked = if pseudo {
            Some(
                "This is space Windows reserves or Roomy couldn't read, not a file you can delete."
                    .into(),
            )
        } else if id == 0 {
            Some("This is the folder you scanned. Delete items inside it instead.".into())
        } else {
            state.guard.check(&path).err()
        };
        Ok(NodeDetails {
            node: views::info(&d.tree, id),
            path: if pseudo {
                String::new()
            } else {
                path.to_string_lossy().into_owned()
            },
            crumbs: views::crumbs(&d.tree, id),
            pseudo,
            blocked,
        })
    })
}

#[tauri::command]
fn largest_files(state: St, limit: Option<usize>) -> Result<Vec<views::LargeFile>, String> {
    with_data(&state, |d| {
        Ok(views::largest_files(
            &d.tree,
            limit.unwrap_or(1000).min(20_000),
        ))
    })
}

#[tauri::command]
fn file_types(state: St) -> Result<Vec<views::TypeStat>, String> {
    with_data(&state, |d| Ok(views::file_types(&d.tree)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemResult {
    id: u32,
    name: String,
    ok: bool,
    error: Option<String>,
    bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteReport {
    mode: Mode,
    results: Vec<ItemResult>,
    /// Bytes that left the scanned files: freed (permanent) or moved to the
    /// Recycle Bin/Trash. Hard-link aware.
    bytes: u64,
    /// Bytes of removed names whose data is still used by another hard link.
    shared: u64,
    summary: Option<Summary>,
}

/// Bytes in the scan that belong to real files (the gap excluded).
fn scanned_total(d: &ScanData) -> u64 {
    let gap: u64 = d
        .tree
        .children(0)
        .filter(|&c| d.tree.nodes[c as usize].kind == KIND_GAP)
        .map(|c| d.tree.nodes[c as usize].size)
        .sum();
    d.tree.nodes[0].size.saturating_sub(gap)
}

/// Bring the tree in line with the disk for each changed item and return,
/// per item, the bytes that really left the scanned files. Hard links are
/// accounted for: removing one name of a file whose data is still reachable
/// through another name frees nothing.
fn settle(d: &mut ScanData, items: &[(u32, PathBuf)]) -> Vec<u64> {
    items
        .iter()
        .map(|(id, p)| {
            let before = scanned_total(d);
            sync_path(d, *id, p);
            before.saturating_sub(scanned_total(d))
        })
        .collect()
}

fn refresh_summary(d: &mut ScanData) {
    // Things moved to (or emptied from) the Recycle Bin change what's listable there.
    let bin = d
        .tree
        .children(0)
        .find(|&c| d.tree.name(c).eq_ignore_ascii_case("$Recycle.Bin"));
    if let Some(bin) = bin {
        let p = d.tree.path(bin);
        sync_path(d, bin, &p);
    }
    reconcile_gap(d);
    let gap_now: u64 = d
        .tree
        .children(0)
        .filter(|&c| d.tree.nodes[c as usize].kind == KIND_GAP)
        .map(|c| d.tree.nodes[c as usize].size)
        .sum();
    d.summary.scanned_bytes = d.tree.nodes[0].size.saturating_sub(gap_now);
    d.summary.scanned_logical = d.tree.nodes[0].logical.saturating_sub(gap_now);
    d.summary.files = d.tree.nodes[0].files as u64;
    d.summary.generation = d.tree.generation;
}

/// After files under `path` changed on disk, bring the tree in line without
/// a full rescan: drop the node if it's gone, otherwise rescan just it.
fn sync_path(d: &mut ScanData, id: u32, path: &Path) {
    // Never rescan a whole drive root (or a pseudo node) from here: that would
    // drop the drive-level accounting. A scanned folder's root is fine.
    if (id == 0 && d.summary.is_volume) || d.tree.get(id).is_none() || d.tree.is_pseudo(id) {
        return;
    }
    match std::fs::symlink_metadata(path) {
        Err(_) => d.tree.remove(id),
        Ok(md) if md.is_dir() && d.tree.nodes[id as usize].kind == KIND_DIR => {
            let name = d.tree.name(id).to_string();
            let (raw, dups) = scan::rescan_dir(path, name, Arc::new(AtomicBool::new(false)));
            d.tree.replace_subtree_with_links(id, raw, dups);
        }
        Ok(_) => {}
    }
}

#[tauri::command]
async fn delete_nodes(app: AppHandle, ids: Vec<u32>, mode: Mode) -> Result<DeleteReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        // Resolve targets under a read lock; never trust paths from the UI.
        let targets: Vec<(u32, String, PathBuf, u64, Option<String>)> = {
            let g = state.data.read();
            let d = g.as_ref().ok_or("No scan yet.")?;
            ids.iter()
                .filter_map(|&id| d.tree.get(id).map(|_| id))
                .map(|id| {
                    let path = d.tree.path(id);
                    let blocked = if id == 0 {
                        Some("Roomy won't delete the folder you scanned.".to_string())
                    } else if d.tree.nodes[id as usize].flags & FLAG_MOUNT != 0 {
                        Some(
                            "Another drive is mounted here. Roomy never removes a mounted drive."
                                .to_string(),
                        )
                    } else if d.tree.is_pseudo(id) {
                        Some("That's reserved system space, not a file.".to_string())
                    } else {
                        None
                    };
                    (
                        id,
                        d.tree.name(id).to_string(),
                        path,
                        d.tree.nodes[id as usize].size,
                        blocked,
                    )
                })
                .collect()
        };
        let mut results = Vec::new();
        for (id, name, path, size, blocked) in &targets {
            let r = match blocked {
                Some(b) => Err(b.clone()),
                None => delete::delete_path(&state.guard, path, mode),
            };
            results.push((*id, name.clone(), path.clone(), *size, r));
        }
        let mut g = state.data.write();
        let d = g.as_mut().ok_or("No scan yet.")?;
        let mut out = Vec::new();
        let mut total = 0u64;
        let mut shared = 0u64;
        let freed = settle(
            d,
            &results
                .iter()
                .map(|(id, _, p, _, _)| (*id, p.clone()))
                .collect::<Vec<_>>(),
        );
        for ((id, name, _path, _listed, r), moved) in results.into_iter().zip(freed) {
            // Permanent: what the remover really gave back (files with no other
            // hard link at the moment they were removed). Trash: what left the
            // scanned files and went to the bin.
            let bytes = match (&r, mode) {
                (Ok(st), Mode::Permanent) => {
                    shared += st.shared_bytes;
                    st.freed_bytes
                }
                _ => moved,
            };
            total += bytes;
            match r {
                Ok(stats) => out.push(ItemResult {
                    id,
                    name,
                    ok: stats.failed == 0,
                    error: stats.first_error.map(|e| {
                        if stats.failed > 1 {
                            format!("{e} ({} items couldn't be removed.)", stats.failed)
                        } else {
                            e
                        }
                    }),
                    bytes,
                }),
                Err(e) => out.push(ItemResult {
                    id,
                    name,
                    ok: false,
                    error: Some(e),
                    bytes,
                }),
            }
        }
        refresh_summary(d);
        Ok(DeleteReport {
            mode,
            results: out,
            bytes: total,
            shared,
            summary: Some(d.summary.clone()),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_suggestions(app: AppHandle) -> Result<Vec<suggest::Suggestion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let elevated = system::is_elevated();
        let mut list = state.pool.install(|| suggest::compute(elevated));
        if let Some(d) = state.data.write().as_mut() {
            let s = storage_for(d);
            if let Some(p) = suggest::project_junk_from(&s.entities, &d.tree) {
                list.push(p);
            }
        }
        *state.suggestions.lock() = list.clone();
        Ok(list)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanSelection {
    suggestion_id: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanReport {
    /// Space really given back: removed files that had no other hard link
    /// at the moment they were removed, plus what emptying the bin freed.
    freed: u64,
    /// Removed names whose data is still used through another hard link.
    shared: u64,
    cleaned: u32,
    failed: u64,
    errors: Vec<String>,
    summary: Option<Summary>,
}

#[tauri::command]
async fn clean_suggestions(
    app: AppHandle,
    selections: Vec<CleanSelection>,
) -> Result<CleanReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let known = state.suggestions.lock().clone();
        let mut freed = 0u64;
        let mut shared = 0u64;
        let mut bin_freed = 0u64;
        let mut cleaned = 0u32;
        let mut failed = 0u64;
        let mut errors: Vec<String> = Vec::new();
        let mut touched: Vec<PathBuf> = Vec::new();
        for sel in selections {
            // Only act on paths Roomy itself suggested.
            let Some(s) = known.iter().find(|s| s.id == sel.suggestion_id) else {
                continue;
            };
            if !s.items.iter().any(|i| i.path == sel.path) || s.action == suggest::Action::Guide {
                continue;
            }
            if s.action == suggest::Action::RecycleBin {
                let before = system::recycle_bin_size().map(|(b, _)| b).unwrap_or(0);
                match system::empty_recycle_bin() {
                    Ok(()) => {
                        let after = system::recycle_bin_size().map(|(b, _)| b).unwrap_or(0);
                        bin_freed += before.saturating_sub(after);
                        cleaned += 1;
                    }
                    Err(e) => errors.push(e),
                }
                continue;
            }
            let path = PathBuf::from(&sel.path);
            let min_age = s.min_age_hours.map(|h| Duration::from_secs(h * 3600));
            let res = match s.action {
                suggest::Action::Contents => delete::delete_contents(&state.guard, &path, min_age),
                _ => delete::delete_path(&state.guard, &path, s.delete_mode),
            };
            match res {
                Ok(stats) => {
                    // Physical: counted per file from its link count at removal.
                    freed += stats.freed_bytes;
                    shared += stats.shared_bytes;
                    cleaned += 1;
                    failed += stats.failed;
                    if let Some(e) = stats.first_error {
                        if errors.len() < 20 {
                            errors.push(e);
                        }
                    }
                }
                Err(e) => errors.push(e),
            }
            touched.push(path);
        }
        // Bring the scanned tree in line for everything we touched.
        let summary = {
            let mut g = state.data.write();
            if let Some(d) = g.as_mut() {
                let items: Vec<(u32, PathBuf)> = touched
                    .iter()
                    .filter_map(|p| views::find_by_path(&d.tree, p).map(|id| (id, p.clone())))
                    .collect();
                settle(d, &items);
                refresh_summary(d);
                Some(d.summary.clone())
            } else {
                None
            }
        };
        Ok(CleanReport {
            freed: freed + bin_freed,
            shared,
            cleaned,
            failed,
            errors,
            summary,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DupeProgressEvent {
    done: u64,
    total: u64,
}

#[tauri::command]
async fn find_duplicates(app: AppHandle) -> Result<Vec<dupes::DupeGroup>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let cancel = Arc::new(AtomicBool::new(false));
        *state.dupes_cancel.lock() = cancel.clone();
        let (cands, mtimes) = {
            let g = state.data.read();
            let d = g.as_ref().ok_or("Scan a drive or folder first.")?;
            let c = dupes::candidates(&d.tree, &state.guard);
            let m: HashMap<u32, i64> = c
                .iter()
                .map(|(id, _, _)| (*id, d.tree.nodes[*id as usize].mtime))
                .collect();
            (c, m)
        };
        let progress = Arc::new(dupes::DupeProgress {
            total_bytes: Default::default(),
            done_bytes: Default::default(),
        });
        let finished = Arc::new(AtomicBool::new(false));
        let emitter = {
            let app = app.clone();
            let progress = progress.clone();
            let finished = finished.clone();
            std::thread::spawn(move || {
                while !finished.load(Ordering::Relaxed) {
                    let _ = app.emit(
                        "dupes-progress",
                        DupeProgressEvent {
                            done: progress.done_bytes.load(Ordering::Relaxed),
                            total: progress.total_bytes.load(Ordering::Relaxed),
                        },
                    );
                    std::thread::sleep(Duration::from_millis(150));
                }
            })
        };
        let groups = state
            .pool
            .install(|| dupes::find(cands, &mtimes, &progress, &cancel));
        finished.store(true, Ordering::Relaxed);
        let _ = emitter.join();
        Ok(groups)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn cancel_duplicates(state: St) {
    state.dupes_cancel.lock().store(true, Ordering::SeqCst);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EntitySummary {
    id: u32,
    category: storage::Category,
    kind: storage::EntityKind,
    name: String,
    detail: Option<String>,
    size: u64,
    buckets: [u64; 4],
    files: u64,
    reclaimable: u64,
    /// What "Clear cache" would really free.
    clearable: u64,
    /// What "Clean build & deps" would really free.
    cleanable: u64,
    can_uninstall: bool,
    can_clear_cache: bool,
    can_clean_build: bool,
    locations: usize,
    mtime: i64,
    /// Key for `get_icons` when Roomy knows where this app's icon lives.
    icon: Option<String>,
}

fn entity_summary(e: &storage::Entity, icons: &icons::IconStore) -> EntitySummary {
    let icon = icons::key(&e.icons);
    if let Some(k) = &icon {
        icons.register(k, &e.icons);
    }
    EntitySummary {
        id: e.id,
        category: e.category,
        kind: e.kind,
        name: e.name.clone(),
        detail: e.detail.clone(),
        size: e.size,
        buckets: e.buckets,
        files: e.files,
        reclaimable: e.reclaimable(),
        clearable: e.reclaim_cache,
        cleanable: e.reclaim_build,
        can_uninstall: e.uninstall.is_some(),
        can_clear_cache: !e.cache_dirs.is_empty() && e.buckets[storage::B_CACHE as usize] > 0,
        can_clean_build: !e.junk_dirs.is_empty() && e.buckets[storage::B_BUILD as usize] > 0,
        locations: e.locations.len(),
        mtime: e.mtime,
        icon,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageView {
    total: u64,
    used: Option<u64>,
    free: Option<u64>,
    capacity: Option<u64>,
    is_volume: bool,
    generation: u64,
    categories: Vec<storage::CategoryTotal>,
    entities: Vec<EntitySummary>,
}

/// Build (or reuse) the storage attribution for the current tree.
fn storage_for(d: &mut ScanData) -> Arc<storage::Storage> {
    if let Some(s) = &d.storage {
        if s.generation == d.tree.generation {
            return s.clone();
        }
    }
    let s = Arc::new(storage::attribute(
        &d.tree,
        &d.catalog,
        &storage::Env::system(),
        d.summary.is_volume,
    ));
    d.storage = Some(s.clone());
    s
}

#[tauri::command]
async fn get_storage(app: AppHandle) -> Result<StorageView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let icons = app.state::<icons::IconStore>();
        let mut g = state.data.write();
        let d = g.as_mut().ok_or("No scan yet.")?;
        let s = storage_for(d);
        let mut entities: Vec<EntitySummary> = s
            .entities
            .iter()
            .filter(|e| e.size > 0)
            .map(|e| entity_summary(e, &icons))
            .collect();
        entities.sort_by_key(|e| std::cmp::Reverse(e.size));
        Ok(StorageView {
            total: s.total,
            used: d.summary.used,
            free: d.summary.volume_free,
            capacity: d.summary.volume_total,
            is_volume: d.summary.is_volume,
            generation: s.generation,
            categories: s.categories.clone(),
            entities,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationView {
    node: u32,
    path: String,
    bucket: u8,
    bytes: u64,
    is_dir: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EntityDetails {
    entity: EntitySummary,
    locations: Vec<LocationView>,
    cache_paths: Vec<(String, u64)>,
    build_paths: Vec<(String, u64)>,
    uninstall: Option<catalog::Uninstall>,
}

/// Icons for entity rows, as data URLs (None: show the letter tile).
/// Resolved off the UI thread on a small low-priority pool and cached.
#[tauri::command]
async fn get_icons(
    app: AppHandle,
    keys: Vec<String>,
) -> Result<HashMap<String, Option<String>>, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<icons::IconStore>().get_many(&keys))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_entity(
    state: St,
    icons: State<'_, icons::IconStore>,
    id: u32,
) -> Result<EntityDetails, String> {
    let mut g = state.data.write();
    let d = g.as_mut().ok_or("No scan yet.")?;
    let s = storage_for(d);
    let e = s
        .entities
        .get(id as usize)
        .ok_or("That item is no longer there.")?;
    let tree = &d.tree;
    let path_of = |n: u32| -> Option<String> {
        tree.get(n)?;
        (!tree.is_pseudo(n)).then(|| tree.path(n).to_string_lossy().into_owned())
    };
    Ok(EntityDetails {
        entity: entity_summary(e, &icons),
        locations: e
            .locations
            .iter()
            .filter(|l| tree.get(l.node).is_some())
            .map(|l| LocationView {
                node: l.node,
                path: path_of(l.node).unwrap_or_else(|| tree.name(l.node).to_string()),
                bucket: l.bucket,
                bytes: l.bytes,
                is_dir: tree.nodes[l.node as usize].kind != KIND_FILE,
            })
            .collect(),
        cache_paths: e
            .cache_dirs
            .iter()
            .filter_map(|&n| Some((path_of(n)?, tree.nodes[n as usize].size)))
            .collect(),
        build_paths: e
            .junk_dirs
            .iter()
            .filter_map(|&n| Some((path_of(n)?, tree.nodes[n as usize].size)))
            .collect(),
        uninstall: e.uninstall.clone(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionReport {
    message: String,
    freed: u64,
    failed: u64,
    summary: Option<Summary>,
}

type Targets = Vec<(u32, PathBuf)>;

#[tauri::command]
async fn entity_action(app: AppHandle, id: u32, action: String) -> Result<ActionReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        // Resolve targets from Roomy's own attribution, never from the UI.
        let (targets, uninstall, locations): (Targets, Option<catalog::Uninstall>, Targets) = {
            let mut g = state.data.write();
            let d = g.as_mut().ok_or("No scan yet.")?;
            let s = storage_for(d);
            let e = s
                .entities
                .get(id as usize)
                .ok_or("That item is no longer there.")?;
            let list = match action.as_str() {
                "clearCache" => e.cache_dirs.clone(),
                "cleanBuild" => e.junk_dirs.clone(),
                _ => Vec::new(),
            };
            let live = |n: &u32| d.tree.get(*n).is_some() && !d.tree.is_pseudo(*n);
            (
                list.iter()
                    .filter(|n| live(n))
                    .map(|&n| (n, d.tree.path(n)))
                    .collect(),
                e.uninstall.clone(),
                e.locations
                    .iter()
                    .map(|l| l.node)
                    .filter(|n| *n != 0 || !d.summary.is_volume)
                    .filter(live)
                    .map(|n| (n, d.tree.path(n)))
                    .collect(),
            )
        };
        match action.as_str() {
            "uninstall" => {
                let u = uninstall.ok_or("Roomy doesn't know how to uninstall this one.")?;
                let message = system::run_uninstall(&u)?;
                Ok(ActionReport {
                    message,
                    freed: 0,
                    failed: 0,
                    summary: None,
                })
            }
            "refresh" => {
                let mut g = state.data.write();
                let d = g.as_mut().ok_or("No scan yet.")?;
                let freed: u64 = settle(d, &locations).iter().sum();
                refresh_summary(d);
                Ok(ActionReport {
                    message: "Updated.".into(),
                    freed,
                    failed: 0,
                    summary: Some(d.summary.clone()),
                })
            }
            "clearCache" | "cleanBuild" => {
                let mut failed = 0;
                let mut errors = Vec::new();
                let mut freed = 0u64;
                let mut shared = 0u64;
                for (_, p) in &targets {
                    let r = if action == "clearCache" {
                        delete::delete_contents(&state.guard, p, None)
                    } else {
                        delete::delete_path(&state.guard, p, Mode::Permanent)
                    };
                    match r {
                        Ok(st) => {
                            failed += st.failed;
                            freed += st.freed_bytes;
                            shared += st.shared_bytes;
                            if let Some(e) = st.first_error {
                                errors.push(e);
                            }
                        }
                        Err(e) => {
                            failed += 1;
                            errors.push(e);
                        }
                    }
                }
                let mut g = state.data.write();
                let d = g.as_mut().ok_or("No scan yet.")?;
                settle(d, &targets);
                refresh_summary(d);
                let shared_note = if shared > 0 {
                    format!(
                        " {} more is still used by other hard links.",
                        fmt_bytes(shared)
                    )
                } else {
                    String::new()
                };
                Ok(ActionReport {
                    message: match errors.first() {
                        Some(e) if failed > 0 => {
                            format!("Freed {}.{shared_note} {e}", fmt_bytes(freed))
                        }
                        _ => format!("Freed {}.{shared_note}", fmt_bytes(freed)),
                    },
                    freed,
                    failed,
                    summary: Some(d.summary.clone()),
                })
            }
            _ => Err("Unknown action.".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState {
        data: RwLock::new(None),
        scanning: AtomicBool::new(false),
        cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
        suggestions: Mutex::new(Vec::new()),
        dupes_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
        guard: safety::Guard::system(),
        pool: Arc::new(scan::scan_pool()),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(state)
        .setup(|app| {
            // Extracted app icons are cached in Roomy's own data folder. When
            // elevated, Roomy keeps them in memory only: it never writes to or
            // cleans a folder the normal user could redirect.
            let dir = if system::is_elevated() {
                None
            } else {
                app.path()
                    .app_local_data_dir()
                    .ok()
                    .map(|d| d.join("icons"))
            };
            app.manage(icons::IconStore::new(dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_volumes,
            relaunch_elevated,
            open_storage_settings,
            start_scan,
            cancel_scan,
            close_scan,
            get_summary,
            get_children,
            get_treemap,
            get_node,
            largest_files,
            file_types,
            delete_nodes,
            get_suggestions,
            clean_suggestions,
            find_duplicates,
            cancel_duplicates,
            get_storage,
            get_entity,
            get_icons,
            entity_action,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Roomy");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_data(root: &Path) -> ScanData {
        let ctx = scan::ScanCtx::new(
            root,
            Arc::new(AtomicBool::new(false)),
            Arc::new(scan::Progress::default()),
        );
        let raw = scan::scan_dir(&ctx, root);
        let dups = std::mem::take(&mut *ctx.dup_keys.lock());
        ScanData {
            tree: Tree::from_raw_with_links(raw, root.to_string_lossy().into_owned(), dups),
            summary: Summary::default(),
            catalog: Arc::new(catalog::Catalog::default()),
            storage: None,
            mft: None,
        }
    }

    #[test]
    fn deleting_one_name_of_a_hard_linked_file_reports_nothing_freed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a/data.bin"), vec![1u8; 80_000]).unwrap();
        std::fs::hard_link(root.join("a/data.bin"), root.join("b/same.bin")).unwrap();
        std::fs::write(root.join("a/solo.bin"), vec![2u8; 40_000]).unwrap();
        let mut d = scan_data(&root);
        if d.tree.dup_keys.is_empty() {
            return; // filesystem without hard-link identity
        }
        let guard = safety::Guard::default();
        let a = d.tree.child_by_name(0, "a").unwrap();
        let linked = d.tree.child_by_name(a, "data.bin").unwrap();
        let solo = d.tree.child_by_name(a, "solo.bin").unwrap();
        let solo_size = d.tree.nodes[solo as usize].size;
        for (id, name) in [(linked, "data.bin"), (solo, "solo.bin")] {
            delete::delete_path(&guard, &root.join("a").join(name), Mode::Permanent).unwrap();
            let _ = id;
        }
        let freed = settle(
            &mut d,
            &[
                (linked, root.join("a/data.bin")),
                (solo, root.join("a/solo.bin")),
            ],
        );
        assert_eq!(freed[0], 0, "the data is still on disk as b/same.bin");
        assert_eq!(freed[1], solo_size, "a file with one name is really freed");
    }

    #[test]
    fn drive_roots_are_never_resynced_but_folder_roots_are() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("x.bin"), vec![1u8; 1000]).unwrap();
        // As a drive scan: the root is never rescanned from here.
        let mut d = scan_data(&root);
        d.summary.is_volume = true;
        std::fs::remove_file(root.join("x.bin")).unwrap();
        sync_path(&mut d, 0, &root);
        assert_eq!(d.tree.nodes[0].logical, 1000, "drive root untouched");
        // As a folder scan (e.g. the folder is itself a cache that was
        // emptied): the root is brought up to date.
        std::fs::write(root.join("x.bin"), vec![1u8; 1000]).unwrap();
        let mut d = scan_data(&root);
        std::fs::remove_file(root.join("x.bin")).unwrap();
        let freed = settle(&mut d, &[(0, root.clone())]);
        assert_eq!(d.tree.nodes[0].logical, 0);
        assert!(freed[0] > 0);
    }

    #[test]
    fn delete_reports_physical_space_even_when_the_other_link_is_outside_the_scan() {
        // Scan only `proj`; its node_modules file is hard-linked to a store
        // outside the scan (like pnpm). Removing it frees nothing.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let proj = base.join("proj");
        std::fs::create_dir_all(proj.join("node_modules")).unwrap();
        std::fs::create_dir_all(base.join("store")).unwrap();
        std::fs::write(base.join("store/dep.js"), vec![7u8; 64 * 1024]).unwrap();
        std::fs::hard_link(base.join("store/dep.js"), proj.join("node_modules/dep.js")).unwrap();
        std::fs::write(proj.join("node_modules/own.js"), vec![8u8; 64 * 1024]).unwrap();
        let stats = delete::delete_path(
            &safety::Guard::default(),
            &proj.join("node_modules"),
            Mode::Permanent,
        )
        .unwrap();
        assert!(stats.shared_bytes >= 64 * 1024 - 1, "{stats:?}");
        assert!(
            stats.freed_bytes >= 64 * 1024 - 1 && stats.freed_bytes < 2 * 64 * 1024,
            "{stats:?}"
        );
        assert!(base.join("store/dep.js").exists());
    }

    #[test]
    fn gap_parts_always_add_up() {
        assert_eq!(split_gap(1000, Some(300), 200), (300, 200, 500));
        assert_eq!(split_gap(100, Some(300), 200), (100, 0, 0));
        assert_eq!(split_gap(250, None, 400), (0, 250, 0));
        assert_eq!(split_gap(0, Some(10), 10), (0, 0, 0));
    }
}
