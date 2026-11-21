//! Roomy: Tauri commands and app state.

pub mod delete;
pub mod dupes;
pub mod safety;
pub mod scan;
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

#[derive(Debug, Clone, Serialize)]
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

fn build_gap(
    path: &Path,
    used: u64,
    scanned: u64,
    denied: u64,
    elevated: bool,
) -> (Option<RawDir>, Vec<GapPart>) {
    let gap = used as i64 - scanned as i64;
    let mut parts = Vec::new();
    if gap <= 0 {
        return (None, parts);
    }
    let gap = gap as u64;
    let mut dir = RawDir::new("System & unreadable space".into(), 0);
    dir.kind = KIND_GAP;
    let mut left = gap;
    if let Some(mft) = system::ntfs_mft_bytes(path) {
        let mft = mft.min(left);
        if mft > 0 {
            left -= mft;
            dir.files.push(RawEntry {
                name: "NTFS file table ($MFT)".into(),
                size: mft,
                logical: mft,
                mtime: 0,
                kind: KIND_GAP_PART,
                flags: 0,
            });
            parts.push(GapPart {
                label: "NTFS file table ($MFT)".into(),
                bytes: Some(mft),
                explain: "The index of every file on the drive. It grows with the number of files and Windows manages it.".into(),
            });
        }
    }
    if left > 0 {
        let explain = if cfg!(windows) {
            let mut s = String::from(
                "Space Windows reports as used that no folder listing shows: System Restore points and shadow copies (in System Volume Information), file system metadata and reserved storage for updates",
            );
            if denied > 0 && !elevated {
                s.push_str(&format!(
                    ", plus {denied} folders Roomy wasn't allowed to open"
                ));
            }
            s.push_str(". Run Roomy as administrator to look inside more of it.");
            s
        } else {
            format!(
                "Space the system reports as used that Roomy couldn't attribute to a file: snapshots, file system metadata, purgeable space{}.",
                if denied > 0 {
                    format!(" and {denied} folders Roomy wasn't allowed to open")
                } else {
                    String::new()
                }
            )
        };
        dir.files.push(RawEntry {
            name: "Restore points, reserved & unreadable".into(),
            size: left,
            logical: left,
            mtime: 0,
            kind: KIND_GAP_PART,
            flags: 0,
        });
        parts.push(GapPart {
            label: "Restore points, reserved & unreadable".into(),
            bytes: Some(left),
            explain,
        });
    }
    dir.finish();
    // Pseudo items are not files.
    dir.total_files = 0;
    (Some(dir), parts)
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
    let mut gap_parts = Vec::new();
    if let Some(used) = used {
        let (gap_dir, parts) = build_gap(&root, used, scanned, denied, elevated);
        if let Some(g) = gap_dir {
            raw.dirs.push(g);
            raw.finish();
        }
        gap_parts = parts;
    }
    let tree = Tree::from_raw(raw, root_name.clone());
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
        gap: used.map(|u| u as i64 - scanned as i64).unwrap_or(0),
        gap_parts,
        hardlink_bytes: ctx.hardlink_bytes.load(Ordering::Relaxed),
        links_skipped: ctx.links_skipped.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().as_millis() as u64,
        elevated,
        generation: tree.generation,
    };
    Some(ScanData { tree, summary })
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
    /// Bytes freed (permanent) or moved to the Recycle Bin/Trash.
    bytes: u64,
    summary: Option<Summary>,
}

fn refresh_summary(d: &mut ScanData) {
    let gap_now: u64 = d
        .tree
        .children(0)
        .filter(|&c| d.tree.nodes[c as usize].kind == KIND_GAP)
        .map(|c| d.tree.nodes[c as usize].size)
        .sum();
    d.summary.scanned_bytes = d.tree.nodes[0].size.saturating_sub(gap_now);
    d.summary.scanned_logical = d.tree.nodes[0].logical.saturating_sub(gap_now);
    d.summary.files = d.tree.nodes[0].files as u64;
    if d.summary.is_volume {
        if let Some(u) = system::disk_usage(Path::new(&d.summary.root_path)) {
            d.summary.volume_free = Some(u.free);
            d.summary.volume_total = Some(u.total);
        }
    }
    d.summary.generation = d.tree.generation;
}

/// After files under `path` changed on disk, bring the tree in line without
/// a full rescan: drop the node if it's gone, otherwise rescan just it.
fn sync_path(d: &mut ScanData, id: u32, path: &Path) {
    if d.tree.get(id).is_none() {
        return;
    }
    match std::fs::symlink_metadata(path) {
        Err(_) => d.tree.remove(id),
        Ok(md) if md.is_dir() && d.tree.nodes[id as usize].kind == KIND_DIR => {
            let name = d.tree.name(id).to_string();
            let raw = scan::rescan_dir(path, name, Arc::new(AtomicBool::new(false)));
            d.tree.replace_subtree(id, raw);
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
        for (id, name, path, before, r) in results {
            sync_path(d, id, &path);
            let after = d.tree.get(id).map(|n| n.size).unwrap_or(0);
            let bytes = before.saturating_sub(after);
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
        if let Some(d) = state.data.read().as_ref() {
            if let Some(p) = suggest::project_junk(&d.tree) {
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
    freed: u64,
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
        let mut cleaned = 0u32;
        let mut failed = 0u64;
        let mut errors: Vec<String> = Vec::new();
        let mut touched: Vec<PathBuf> = Vec::new();
        for sel in selections {
            // Only act on paths Roomy itself suggested.
            let Some(s) = known.iter().find(|s| s.id == sel.suggestion_id) else {
                continue;
            };
            let Some(item) = s.items.iter().find(|i| i.path == sel.path) else {
                continue;
            };
            if s.action == suggest::Action::Guide {
                continue;
            }
            if s.action == suggest::Action::RecycleBin {
                match system::empty_recycle_bin() {
                    Ok(()) => {
                        freed += item.size;
                        cleaned += 1;
                    }
                    Err(e) => errors.push(e),
                }
                continue;
            }
            let path = PathBuf::from(&item.path);
            let min_age = s.min_age_hours.map(|h| Duration::from_secs(h * 3600));
            let res = match s.action {
                suggest::Action::Contents => delete::delete_contents(&state.guard, &path, min_age),
                _ => delete::delete_path(&state.guard, &path, s.delete_mode),
            };
            match res {
                Ok(stats) => {
                    let (after, _) = scan::measure(&path, Arc::new(AtomicBool::new(false)));
                    freed += item.size.saturating_sub(after);
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
        // Update the scanned tree for everything we touched.
        let summary = {
            let mut g = state.data.write();
            if let Some(d) = g.as_mut() {
                for p in &touched {
                    if let Some(id) = views::find_by_path(&d.tree, p) {
                        sync_path(d, id, p);
                    }
                }
                refresh_summary(d);
                Some(d.summary.clone())
            } else {
                None
            }
        };
        Ok(CleanReport {
            freed,
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
        .manage(state)
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Roomy");
}
