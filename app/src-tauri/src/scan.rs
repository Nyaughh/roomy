//! Parallel filesystem scanner.
//!
//! Walks a directory tree with a work-stealing thread pool. On Windows it
//! enumerates directories with `GetFileInformationByHandleEx`
//! (`FileIdBothDirectoryInfo`), which returns sizes, attributes and file IDs
//! for a whole directory in a few syscalls without opening each file.
//!
//! Correctness rules:
//! * Junctions, symlinks and mount points are recorded but never followed,
//!   so nothing outside the chosen tree is counted and loops are impossible.
//! * Hard links are counted once (deduplicated by file ID / inode).
//! * Directories that can't be read are flagged, not silently dropped.
//! * Entries that vanish mid-scan are skipped.

use crate::tree::*;
use parking_lot::Mutex;
use rayon::prelude::*;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// One directory entry as reported by the platform.
#[derive(Debug)]
pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
    /// Symlink, junction or other name-surrogate reparse point.
    pub is_link: bool,
    /// A different filesystem is mounted here.
    pub is_mount: bool,
    pub alloc: u64,
    pub logical: u64,
    pub mtime: i64,
    /// Key used to count hard-linked files once, if the file may be linked.
    pub dedup_key: Option<u64>,
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub denied: AtomicU64,
    pub current: Mutex<String>,
}

#[derive(Default)]
struct IdHasher(u64);
impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        // splitmix64 finaliser: cheap and well distributed for file ids.
        let mut z = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*b);
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }
}

type IdSet = HashSet<u64, BuildHasherDefault<IdHasher>>;

/// Sharded set of file ids seen so far, for hard-link deduplication.
struct SeenIds {
    shards: Vec<Mutex<IdSet>>,
}

impl SeenIds {
    fn new() -> Self {
        SeenIds {
            shards: (0..64).map(|_| Mutex::new(IdSet::default())).collect(),
        }
    }
    /// Returns true if `id` was not seen before.
    fn insert(&self, id: u64) -> bool {
        let mut h = IdHasher(id);
        h.write_u64(id);
        let shard = (h.finish() >> 58) as usize;
        self.shards[shard].lock().insert(id)
    }
}

pub struct ScanCtx {
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<Progress>,
    seen: SeenIds,
    dedup: bool,
    #[cfg_attr(not(unix), allow(dead_code))]
    root_dev: u64,
    pub denied_paths: Mutex<Vec<String>>,
    pub hardlink_bytes: AtomicU64,
    pub links_skipped: AtomicU64,
}

impl ScanCtx {
    pub fn new(root: &Path, cancel: Arc<AtomicBool>, progress: Arc<Progress>) -> Self {
        ScanCtx {
            cancel,
            progress,
            seen: SeenIds::new(),
            dedup: platform::supports_dedup(root),
            root_dev: platform::device_of(root),
            denied_paths: Mutex::new(Vec::new()),
            hardlink_bytes: AtomicU64::new(0),
            links_skipped: AtomicU64::new(0),
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Build a rayon pool suited to I/O-heavy directory enumeration.
pub fn scan_pool() -> rayon::ThreadPool {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    rayon::ThreadPoolBuilder::new()
        .num_threads((cpus * 2).clamp(4, 32))
        .stack_size(16 * 1024 * 1024)
        .thread_name(|i| format!("roomy-scan-{i}"))
        .build()
        .expect("failed to build scan thread pool")
}

/// Scan `root` (a directory) and return its raw tree.
pub fn scan_dir(ctx: &ScanCtx, root: &Path) -> RawDir {
    let mtime = std::fs::metadata(root)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(system_time_secs)
        .unwrap_or(0);
    walk(
        ctx,
        root.to_path_buf(),
        root.to_string_lossy().into_owned(),
        mtime,
    )
}

fn walk(ctx: &ScanCtx, path: PathBuf, name: String, mtime: i64) -> RawDir {
    let mut dir = RawDir::new(name, mtime);
    if ctx.cancelled() {
        return dir;
    }
    ctx.progress.dirs.fetch_add(1, Ordering::Relaxed);
    if let Some(mut cur) = ctx.progress.current.try_lock() {
        cur.clear();
        cur.push_str(&path.to_string_lossy());
    }

    let entries = match platform::read_entries(&path, ctx) {
        Ok(e) => e,
        Err(e) => {
            if is_vanished(&e) {
                return dir;
            }
            dir.flags |= FLAG_DENIED;
            ctx.progress.denied.fetch_add(1, Ordering::Relaxed);
            let mut d = ctx.denied_paths.lock();
            if d.len() < 500 {
                d.push(path.to_string_lossy().into_owned());
            }
            return dir;
        }
    };

    let mut subdirs: Vec<(String, i64)> = Vec::new();
    let mut bytes = 0u64;
    let mut nfiles = 0u64;
    for e in entries {
        if e.is_link || e.is_mount {
            ctx.links_skipped.fetch_add(1, Ordering::Relaxed);
            dir.files.push(RawEntry {
                name: e.name,
                size: 0,
                logical: 0,
                mtime: e.mtime,
                kind: KIND_LINK,
                flags: if e.is_mount { FLAG_MOUNT } else { 0 },
            });
            continue;
        }
        if e.is_dir {
            subdirs.push((e.name, e.mtime));
            continue;
        }
        let mut flags = 0;
        let mut size = e.alloc;
        let mut logical = e.logical;
        if ctx.dedup {
            if let Some(key) = e.dedup_key {
                if !ctx.seen.insert(key) {
                    ctx.hardlink_bytes.fetch_add(size, Ordering::Relaxed);
                    flags |= FLAG_HARDLINK;
                    size = 0;
                    logical = 0;
                }
            }
        }
        bytes += size;
        nfiles += 1;
        dir.files.push(RawEntry {
            name: e.name,
            size,
            logical,
            mtime: e.mtime,
            kind: KIND_FILE,
            flags,
        });
    }
    ctx.progress.files.fetch_add(nfiles, Ordering::Relaxed);
    ctx.progress.bytes.fetch_add(bytes, Ordering::Relaxed);

    dir.dirs = if subdirs.len() <= 1 {
        subdirs
            .into_iter()
            .map(|(n, m)| walk(ctx, path.join(&n), n, m))
            .collect()
    } else {
        subdirs
            .into_par_iter()
            .map(|(n, m)| walk(ctx, path.join(&n), n, m))
            .collect()
    };
    dir.finish();
    dir
}

fn is_vanished(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::NotFound {
        return true;
    }
    // ERROR_PATH_NOT_FOUND, ERROR_DIRECTORY (became a file), ERROR_DELETE_PENDING
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(3) | Some(267) | Some(303)) {
        return true;
    }
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOTDIR) {
        return true;
    }
    false
}

pub fn system_time_secs(t: std::time::SystemTime) -> i64 {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// Quickly measure the on-disk size of a path (file or directory) using the
/// same rules as the full scan. Returns (bytes, files).
pub fn measure(path: &Path, cancel: Arc<AtomicBool>) -> (u64, u64) {
    let md = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return (0, 0),
    };
    if md.file_type().is_symlink() {
        return (0, 0);
    }
    let ctx = ScanCtx::new(path, cancel, Arc::new(Progress::default()));
    if md.is_dir() {
        let raw = scan_dir(&ctx, path);
        (raw.total_size, raw.total_files)
    } else {
        (platform::file_alloc_size(path, &md), 1)
    }
}

/// Scan a single directory into a RawDir rooted at `path` with the given name.
pub fn rescan_dir(path: &Path, name: String, cancel: Arc<AtomicBool>) -> RawDir {
    let ctx = ScanCtx::new(path, cancel, Arc::new(Progress::default()));
    let mut raw = scan_dir(&ctx, path);
    raw.name = name;
    raw
}

#[cfg(windows)]
pub mod platform {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;

    /// Name-surrogate reparse tags (junctions, symlinks, …) have bit 29 set.
    pub fn is_name_surrogate(tag: u32) -> bool {
        tag & 0x2000_0000 != 0
    }

    /// Convert a path to a NUL-terminated wide string with the `\\?\` prefix
    /// so very long paths work.
    pub fn to_wide_long(path: &Path) -> Vec<u16> {
        let s = path.as_os_str().to_string_lossy().replace('/', "\\");
        let full = if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
            s
        } else if let Some(rest) = s.strip_prefix(r"\\") {
            format!(r"\\?\UNC\{rest}")
        } else {
            format!(r"\\?\{s}")
        };
        std::ffi::OsStr::new(&full)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn to_wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn filetime_to_unix(ft: i64) -> i64 {
        if ft <= 0 {
            return 0;
        }
        ft / 10_000_000 - 11_644_473_600
    }

    pub fn read_entries(path: &Path, _ctx: &ScanCtx) -> io::Result<Vec<DirEntryInfo>> {
        let wide = to_wide_long(path);
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let h = Handle(h);
        // u64 storage keeps the buffer 8-byte aligned as the API requires.
        let mut buf = vec![0u64; 64 * 1024 / 8];
        let buf_bytes = (buf.len() * 8) as u32;
        let mut class = FileIdBothDirectoryRestartInfo;
        let mut out = Vec::new();
        loop {
            let ok = unsafe {
                GetFileInformationByHandleEx(h.0, class, buf.as_mut_ptr() as *mut c_void, buf_bytes)
            };
            class = FileIdBothDirectoryInfo;
            if ok == 0 {
                let err = unsafe { GetLastError() };
                if err == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            let base = buf.as_ptr() as *const u8;
            let mut off = 0usize;
            loop {
                // SAFETY: the API fills `buf` with a chain of
                // FILE_ID_BOTH_DIR_INFO records linked by NextEntryOffset.
                let p = unsafe { base.add(off) } as *const FILE_ID_BOTH_DIR_INFO;
                let info = unsafe { std::ptr::read_unaligned(p) };
                let name_len = info.FileNameLength as usize / 2;
                let name_ptr = unsafe { std::ptr::addr_of!((*p).FileName) } as *const u16;
                let name_units = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
                let is_dot =
                    name_units == [b'.' as u16] || name_units == [b'.' as u16, b'.' as u16];
                if !is_dot {
                    let attrs = info.FileAttributes;
                    let is_dir = attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
                    let is_reparse = attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                    // For reparse points EaSize holds the reparse tag.
                    let is_link = is_reparse && is_name_surrogate(info.EaSize);
                    let file_id = info.FileId as u64;
                    out.push(DirEntryInfo {
                        name: String::from_utf16_lossy(name_units),
                        is_dir,
                        is_link,
                        is_mount: false,
                        alloc: info.AllocationSize.max(0) as u64,
                        logical: info.EndOfFile.max(0) as u64,
                        mtime: filetime_to_unix(info.LastWriteTime),
                        dedup_key: if is_dir || file_id == 0 {
                            None
                        } else {
                            Some(file_id)
                        },
                    });
                }
                if info.NextEntryOffset == 0 {
                    break;
                }
                off += info.NextEntryOffset as usize;
            }
        }
        Ok(out)
    }

    /// File IDs are only stable, unique identifiers on NTFS.
    pub fn supports_dedup(root: &Path) -> bool {
        volume_fs_name(root)
            .map(|fs| fs.eq_ignore_ascii_case("NTFS"))
            .unwrap_or(false)
    }

    pub fn device_of(_root: &Path) -> u64 {
        0
    }

    /// Volume mount point (e.g. `C:\`) containing `path`.
    pub fn volume_root(path: &Path) -> Option<String> {
        let wide = to_wide(&path.to_string_lossy());
        let mut buf = vec![0u16; 1024];
        let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn volume_fs_name(path: &Path) -> Option<String> {
        let root = volume_root(path)?;
        let wroot = to_wide(&root);
        let mut fs = vec![0u16; 64];
        let ok = unsafe {
            GetVolumeInformationW(
                wroot.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let len = fs.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&fs[..len]))
    }

    pub fn file_alloc_size(path: &Path, md: &std::fs::Metadata) -> u64 {
        use windows_sys::Win32::Storage::FileSystem::{
            FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
        };
        let wide = to_wide_long(path);
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return md.len();
        }
        let h = Handle(h);
        let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.0,
                FileStandardInfo,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        };
        if ok == 0 {
            md.len()
        } else {
            info.AllocationSize.max(0) as u64
        }
    }
}

#[cfg(unix)]
pub mod platform {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub fn read_entries(path: &Path, ctx: &ScanCtx) -> io::Result<Vec<DirEntryInfo>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            // DirEntry::metadata does not follow symlinks.
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue, // vanished mid-scan
            };
            let ft = md.file_type();
            let is_dir = ft.is_dir();
            let is_mount = is_dir && md.dev() != ctx.root_dev;
            let dedup_key = if !is_dir && md.nlink() > 1 {
                Some((md.dev()).rotate_left(40) ^ md.ino())
            } else {
                None
            };
            out.push(DirEntryInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir,
                is_link: ft.is_symlink(),
                is_mount,
                alloc: md.blocks() * 512,
                logical: md.len(),
                mtime: md.mtime(),
                dedup_key,
            });
        }
        Ok(out)
    }

    pub fn supports_dedup(_root: &Path) -> bool {
        true
    }

    pub fn device_of(root: &Path) -> u64 {
        std::fs::metadata(root).map(|m| m.dev()).unwrap_or(0)
    }

    pub fn file_alloc_size(_path: &Path, md: &std::fs::Metadata) -> u64 {
        md.blocks() * 512
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scan(path: &Path) -> (RawDir, ScanCtx) {
        let ctx = ScanCtx::new(
            path,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Progress::default()),
        );
        let pool = scan_pool();
        let raw = pool.install(|| scan_dir(&ctx, path));
        (raw, ctx)
    }

    fn logical_total(d: &RawDir) -> u64 {
        d.total_logical
    }

    #[test]
    fn counts_files_and_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.bin"), vec![1u8; 10_000]).unwrap();
        fs::create_dir_all(tmp.path().join("x/y/z")).unwrap();
        fs::write(tmp.path().join("x/b.bin"), vec![1u8; 2_000]).unwrap();
        fs::write(tmp.path().join("x/y/z/c.bin"), vec![1u8; 300]).unwrap();
        let (raw, ctx) = scan(tmp.path());
        assert_eq!(raw.total_files, 3);
        assert_eq!(logical_total(&raw), 12_300);
        assert!(raw.total_size >= 10_000, "allocated size should cover data");
        assert_eq!(ctx.progress.files.load(Ordering::Relaxed), 3);
        assert_eq!(ctx.progress.dirs.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn hard_links_counted_once() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("orig.bin");
        fs::write(&a, vec![7u8; 50_000]).unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::hard_link(&a, tmp.path().join("sub/link1.bin")).unwrap();
        fs::hard_link(&a, tmp.path().join("link2.bin")).unwrap();
        let (raw, ctx) = scan(tmp.path());
        if !ctx.dedup {
            // Filesystem without stable ids (e.g. FAT temp dir): nothing to check.
            return;
        }
        assert_eq!(raw.total_logical, 50_000, "hard links must be counted once");
        assert_eq!(raw.total_files, 3, "every link is still listed");
        let flagged = raw
            .files
            .iter()
            .chain(raw.dirs.iter().flat_map(|d| d.files.iter()))
            .filter(|f| f.flags & FLAG_HARDLINK != 0)
            .count();
        assert_eq!(flagged, 2);
    }

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) -> bool {
        // Junctions don't need admin rights or developer mode.
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn make_dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[test]
    fn links_are_not_followed() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("huge.bin"), vec![1u8; 80_000]).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("small.bin"), vec![1u8; 100]).unwrap();
        // A link pointing outside the tree, and one pointing back at the root
        // (a loop). Neither may be followed or counted.
        assert!(make_dir_link(outside.path(), &tmp.path().join("out_link")));
        assert!(make_dir_link(tmp.path(), &tmp.path().join("loop_link")));
        let (raw, ctx) = scan(tmp.path());
        assert_eq!(raw.total_logical, 100);
        assert_eq!(raw.total_files, 1);
        assert_eq!(ctx.links_skipped.load(Ordering::Relaxed), 2);
        let links: Vec<_> = raw.files.iter().filter(|f| f.kind == KIND_LINK).collect();
        assert_eq!(links.len(), 2);
        assert!(raw.dirs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_symlinks_are_not_counted() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("real.bin"), vec![1u8; 4_000]).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real.bin"), tmp.path().join("ln")).unwrap();
        let (raw, _) = scan(tmp.path());
        assert_eq!(raw.total_logical, 4_000);
        assert_eq!(raw.total_files, 1);
    }

    #[test]
    fn missing_root_is_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("nope");
        let (raw, ctx) = scan(&gone);
        assert_eq!(raw.total_files, 0);
        assert_eq!(ctx.progress.denied.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn measure_matches_scan() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("d")).unwrap();
        fs::write(tmp.path().join("d/f.bin"), vec![1u8; 9_000]).unwrap();
        let (bytes, files) = measure(&tmp.path().join("d"), Arc::new(AtomicBool::new(false)));
        assert_eq!(files, 1);
        assert!(bytes >= 9_000);
    }
}
