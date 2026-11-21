//! Deleting files and folders: Recycle Bin/Trash by default, permanent only
//! when asked.
//!
//! Permanent removal works on pinned handles, never on re-resolved path
//! strings: every directory is opened without following links and kept
//! open (Windows: without delete sharing, so it can't be renamed or swapped;
//! Unix: as a directory fd that children are removed relative to). A link is
//! only ever removed as a link, and removal never crosses into another
//! mounted volume.

use crate::safety::{is_protected_name, linked_ancestor, Guard};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Move to the Recycle Bin / Trash (recoverable).
    Trash,
    /// Remove immediately (not recoverable).
    Permanent,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub removed: u64,
    pub failed: u64,
    pub first_error: Option<String>,
}

impl Stats {
    fn fail(&mut self, path: &Path, e: &io::Error) {
        self.failed += 1;
        if self.first_error.is_none() {
            self.first_error = Some(describe(path, e));
        }
    }

    fn fail_msg(&mut self, msg: impl Into<String>) {
        self.failed += 1;
        if self.first_error.is_none() {
            self.first_error = Some(msg.into());
        }
    }
}

pub fn describe(path: &Path, e: &io::Error) -> String {
    let what = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match e.kind() {
        io::ErrorKind::PermissionDenied => {
            format!("“{what}” is in use or needs administrator rights, so it was skipped.")
        }
        _ => match e.raw_os_error() {
            // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
            Some(32) | Some(33) if cfg!(windows) => {
                format!("“{what}” is open in another app, so it was skipped.")
            }
            _ => format!("Couldn't remove “{what}”: {e}"),
        },
    }
}

fn is_link(md: &fs::Metadata) -> bool {
    // On Windows std reports junctions and symlinks (name-surrogate reparse
    // points) as symlinks.
    md.file_type().is_symlink()
}

fn older_than_secs(mtime_secs: i64, min_age: Option<Duration>) -> bool {
    match min_age {
        None => true,
        Some(age) => {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            now - mtime_secs >= age.as_secs() as i64
        }
    }
}

/// Where `path`'s parent really is, plus its name: what removal must find
/// when it opens the target (checked again on the opened handle).
fn expected_location(path: &Path) -> Option<PathBuf> {
    let (parent, name) = (path.parent()?, path.file_name()?);
    Some(fs::canonicalize(parent).ok()?.join(name))
}

/// True if another volume is mounted at `path` (or `path` is a volume root).
pub fn is_mount_point(path: &Path) -> bool {
    sys::is_mount_point(path)
}

/// Walk a folder (never following links) and refuse if it holds something
/// that must never be deleted: a registry hive or system file, or another
/// drive mounted inside it. Run before either kind of delete.
pub fn inspect(dir: &Path) -> Result<(), String> {
    if is_mount_point(dir) {
        return Err(format!("Another drive is mounted at {}.", dir.display()));
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            // Can't look inside: can't promise it's safe.
            Err(e) => return Err(describe(&d, &e)),
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_protected_name(&name) {
                return Err(format!(
                    "It contains “{name}” ({}), a system file that must never be deleted.",
                    p.display()
                ));
            }
            let Ok(md) = fs::symlink_metadata(&p) else {
                continue;
            };
            if (is_link(&md) || md.is_dir()) && is_mount_point(&p) {
                return Err(format!(
                    "Another drive is mounted inside it at {}.",
                    p.display()
                ));
            }
            if md.is_dir() && !is_link(&md) {
                stack.push(p);
            }
        }
    }
    Ok(())
}

/// Permanently remove `path`: a file, a link (removed as a link) or a real
/// directory and everything in it. Items that can't be removed (in use, no
/// permission, on another volume, protected) are skipped and counted.
pub fn remove_tree(path: &Path, min_age: Option<Duration>, stats: &mut Stats) {
    let Some(expected) = expected_location(path) else {
        if fs::symlink_metadata(path).is_ok() {
            stats.fail_msg(format!(
                "Couldn't confirm where {} is, so it was left alone.",
                path.display()
            ));
        }
        return;
    };
    sys::remove_path(path, &expected, min_age, stats);
}

/// Delete one item after checking it against the guard.
pub fn delete_path(guard: &Guard, path: &Path, mode: Mode) -> Result<Stats, String> {
    guard.check(path)?;
    let md = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Stats::default()),
        Err(e) => return Err(describe(path, &e)),
    };
    if (is_link(&md) || md.is_dir()) && is_mount_point(path) {
        return Err("Another drive is mounted here. Roomy never removes a mounted drive.".into());
    }
    if is_link(&md) {
        if mode == Mode::Trash {
            // Links can't go to the Recycle Bin/Trash reliably, and removing one
            // outright isn't recoverable: make that an explicit choice.
            return Err("This is a shortcut link (junction or symlink). Choose “Delete permanently” to remove just the link; what it points to is never touched.".into());
        }
    } else if md.is_dir() {
        // Both modes remove the whole folder, so check everything inside first.
        inspect(path)?;
    }
    match mode {
        Mode::Permanent => {
            let mut stats = Stats::default();
            remove_tree(path, None, &mut stats);
            Ok(stats)
        }
        Mode::Trash => {
            trash::delete(path).map_err(|e| {
                format!(
                    "Couldn't move “{}” to the {}: {e}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    if cfg!(windows) {
                        "Recycle Bin"
                    } else {
                        "Trash"
                    }
                )
            })?;
            Ok(Stats {
                removed: 1,
                ..Default::default()
            })
        }
    }
}

/// Permanently delete everything *inside* `dir` (the folder itself stays).
/// Files newer than `min_age` are kept. Used for caches and temp folders.
pub fn delete_contents(
    guard: &Guard,
    dir: &Path,
    min_age: Option<Duration>,
) -> Result<Stats, String> {
    let md = match fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Stats::default()),
        Err(e) => return Err(describe(dir, &e)),
    };
    if is_link(&md) {
        return Err(format!(
            "{} is a link to somewhere else, so Roomy left it alone.",
            dir.display()
        ));
    }
    if let Some(link) = linked_ancestor(dir) {
        return Err(format!(
            "{} is reached through a link ({}), so Roomy left it alone.",
            dir.display(),
            link.display()
        ));
    }
    if is_mount_point(dir) {
        return Err(format!(
            "Another drive is mounted at {}, so Roomy left it alone.",
            dir.display()
        ));
    }
    let mut stats = Stats::default();
    if !md.is_dir() {
        guard.check(dir)?;
        remove_tree(dir, min_age, &mut stats);
        return Ok(stats);
    }
    let mut check = |child: &Path| -> Result<(), String> {
        guard.check(child)?;
        let is_real_dir = fs::symlink_metadata(child)
            .map(|m| m.is_dir() && !is_link(&m))
            .unwrap_or(false);
        if is_real_dir {
            inspect(child)?;
        }
        Ok(())
    };
    sys::remove_children(dir, min_age, &mut stats, &mut check).map_err(|e| describe(dir, &e))?;
    Ok(stats)
}

type Check<'a> = dyn FnMut(&Path) -> Result<(), String> + 'a;

#[cfg(windows)]
mod sys {
    use super::*;
    use crate::scan::platform::{enumerate_handle, filetime_to_unix, to_wide_long, volume_root};
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;

    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const ERROR_DIR_NOT_EMPTY: i32 = 145;

    struct H(HANDLE);
    impl Drop for H {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// Open without following a final link, without delete sharing (so the
    /// object can't be renamed or replaced while held), with delete access.
    fn open(path: &Path) -> io::Result<H> {
        let w = to_wide_long(path);
        let try_open = |access: u32| unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        let base = DELETE | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES;
        let mut h = try_open(base | FILE_LIST_DIRECTORY);
        if h == INVALID_HANDLE_VALUE {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::PermissionDenied {
                return Err(e);
            }
            h = try_open(base);
            if h == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(H(h))
    }

    fn attr_tag(h: &H) -> io::Result<(u32, u32)> {
        let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.0,
                FileAttributeTagInfo,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((info.FileAttributes, info.ReparseTag))
    }

    fn basic(h: &H) -> io::Result<FILE_BASIC_INFO> {
        let mut info: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.0,
                FileBasicInfo,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    /// Delete the object behind the handle (POSIX semantics where available).
    fn delete(h: &H) -> io::Result<()> {
        let ex = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        let ok = unsafe {
            SetFileInformationByHandle(
                h.0,
                FileDispositionInfoEx,
                &ex as *const _ as *const c_void,
                std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // Older Windows/filesystems: clear read-only, then classic delete.
        if !matches!(e.raw_os_error(), Some(1) | Some(50) | Some(87)) {
            return Err(e);
        }
        if let Ok(mut b) = basic(h) {
            if b.FileAttributes & FILE_ATTRIBUTE_READONLY != 0 {
                b.FileAttributes &= !FILE_ATTRIBUTE_READONLY;
                b.CreationTime = 0;
                b.LastAccessTime = 0;
                b.LastWriteTime = 0;
                b.ChangeTime = 0;
                unsafe {
                    SetFileInformationByHandle(
                        h.0,
                        FileBasicInfo,
                        &b as *const _ as *const c_void,
                        std::mem::size_of::<FILE_BASIC_INFO>() as u32,
                    );
                }
            }
        }
        let d = FILE_DISPOSITION_INFO { DeleteFile: 1 };
        let ok = unsafe {
            SetFileInformationByHandle(
                h.0,
                FileDispositionInfo,
                &d as *const _ as *const c_void,
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn final_path(h: &H) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let n = unsafe { GetFinalPathNameByHandleW(h.0, buf.as_mut_ptr(), buf.len() as u32, 0) };
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }

    fn norm(s: &str) -> String {
        let s = s
            .strip_prefix(r"\\?\UNC\")
            .map(|r| format!(r"\\{r}"))
            .unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(s).to_string());
        s.trim_end_matches('\\').to_lowercase()
    }

    pub fn is_mount_point(path: &Path) -> bool {
        // A drive mounted into a folder, or the root of a volume.
        let via_link = fs::read_link(path)
            .map(|t| t.to_string_lossy().contains("Volume{"))
            .unwrap_or(false);
        let is_volume_root = volume_root(path)
            .map(|r| norm(&r) == norm(&path.to_string_lossy()))
            .unwrap_or(false);
        via_link || is_volume_root
    }

    fn rm(path: &Path, h: H, min_age: Option<Duration>, stats: &mut Stats) {
        let (attrs, tag) = match attr_tag(&h) {
            Ok(v) => v,
            Err(e) => return stats.fail(path, &e),
        };
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 && tag & 0x2000_0000 != 0 {
            // A link: remove the link itself, never what it points to.
            if tag == IO_REPARSE_TAG_MOUNT_POINT && is_mount_point(path) {
                return stats.fail_msg(format!(
                    "Skipped {}: another drive is mounted there.",
                    path.display()
                ));
            }
            match delete(&h) {
                Ok(()) => stats.removed += 1,
                Err(e) => stats.fail(path, &e),
            }
            return;
        }
        if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
            // `h` stays open (no delete sharing) while children are removed, so
            // this folder can't be swapped for something else meanwhile.
            match enumerate_handle(h.0) {
                Ok(entries) => {
                    for e in entries {
                        let child = path.join(&e.name);
                        match open(&child) {
                            Ok(ch) => rm(&child, ch, min_age, stats),
                            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                            Err(err) => stats.fail(&child, &err),
                        }
                    }
                }
                Err(e) => return stats.fail(path, &e),
            }
            match delete(&h) {
                Ok(()) => stats.removed += 1,
                Err(e) if e.raw_os_error() == Some(ERROR_DIR_NOT_EMPTY) => {} // something inside was kept
                Err(e) => stats.fail(path, &e),
            }
            return;
        }
        if path
            .file_name()
            .map(|n| is_protected_name(&n.to_string_lossy()))
            .unwrap_or(false)
        {
            return stats.fail_msg(format!("Kept {}: it's a system file.", path.display()));
        }
        if min_age.is_some() {
            let young = basic(&h)
                .map(|b| !older_than_secs(filetime_to_unix(b.LastWriteTime), min_age))
                .unwrap_or(true);
            if young {
                return;
            }
        }
        match delete(&h) {
            Ok(()) => stats.removed += 1,
            Err(e) => stats.fail(path, &e),
        }
    }

    pub fn remove_path(path: &Path, expected: &Path, min_age: Option<Duration>, stats: &mut Stats) {
        let h = match open(path) {
            Ok(h) => h,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => return stats.fail(path, &e),
        };
        // The object we pinned must be the one that was checked.
        if final_path(&h)
            .map(|f| norm(&f) != norm(&expected.to_string_lossy()))
            .unwrap_or(true)
        {
            return stats.fail_msg(format!(
                "{} changed while Roomy was working, so it was left alone.",
                path.display()
            ));
        }
        rm(path, h, min_age, stats);
    }

    pub fn remove_children(
        dir: &Path,
        min_age: Option<Duration>,
        stats: &mut Stats,
        check: &mut Check<'_>,
    ) -> io::Result<()> {
        let h = open(dir)?;
        let (attrs, _) = attr_tag(&h)?;
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 || attrs & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(io::Error::other("not a real folder"));
        }
        // `h` pins the folder for the whole operation.
        for e in enumerate_handle(h.0)? {
            let child = dir.join(&e.name);
            if let Err(reason) = check(&child) {
                stats.fail_msg(reason);
                continue;
            }
            match open(&child) {
                Ok(ch) => rm(&child, ch, min_age, stats),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => stats.fail(&child, &err),
            }
        }
        drop(h);
        Ok(())
    }
}

#[cfg(unix)]
mod sys {
    use super::*;
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::MetadataExt;

    fn cstr(s: &OsStr) -> io::Result<CString> {
        CString::new(s.as_bytes()).map_err(|_| io::Error::other("bad name"))
    }

    fn last() -> io::Error {
        io::Error::last_os_error()
    }

    fn lstat_at(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            return Err(last());
        }
        Ok(st)
    }

    fn fstat(fd: RawFd) -> io::Result<libc::stat> {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return Err(last());
        }
        Ok(st)
    }

    fn open_dir_at(dir: RawFd, name: &CStr) -> io::Result<OwnedFd> {
        let fd = unsafe {
            libc::openat(
                dir,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(last());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn open_dir(path: &Path, follow: bool) -> io::Result<OwnedFd> {
        let c = cstr(path.as_os_str())?;
        let mut flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        if !follow {
            flags |= libc::O_NOFOLLOW;
        }
        let fd = unsafe { libc::open(c.as_ptr(), flags) };
        if fd < 0 {
            return Err(last());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn list(fd: RawFd) -> io::Result<Vec<OsString>> {
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 {
            return Err(last());
        }
        let dirp = unsafe { libc::fdopendir(dup) };
        if dirp.is_null() {
            unsafe { libc::close(dup) };
            return Err(last());
        }
        let mut out = Vec::new();
        loop {
            let ent = unsafe { libc::readdir(dirp) };
            if ent.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) }
                .to_bytes()
                .to_vec();
            if name != b"." && name != b".." {
                out.push(OsString::from_vec(name));
            }
        }
        unsafe { libc::closedir(dirp) };
        Ok(out)
    }

    fn unlink_at(dir: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<()> {
        if unsafe { libc::unlinkat(dir, name.as_ptr(), flags) } != 0 {
            return Err(last());
        }
        Ok(())
    }

    fn is_dir(st: &libc::stat) -> bool {
        st.st_mode & libc::S_IFMT == libc::S_IFDIR
    }

    // dev_t/ino_t/time_t widths differ by platform.
    #[allow(clippy::unnecessary_cast)]
    fn ids(st: &libc::stat) -> (u64, u64) {
        (st.st_dev as u64, st.st_ino as u64)
    }

    #[allow(clippy::unnecessary_cast)]
    fn mtime(st: &libc::stat) -> i64 {
        st.st_mtime as i64
    }

    pub fn is_mount_point(path: &Path) -> bool {
        let (Ok(md), Some(parent)) = (fs::symlink_metadata(path), path.parent()) else {
            return false;
        };
        if md.file_type().is_symlink() {
            return false;
        }
        fs::metadata(parent)
            .map(|p| p.dev() != md.dev())
            .unwrap_or(false)
    }

    /// Remove `name` inside the pinned directory `dir`, never leaving `dev`.
    fn rm_at(
        dir: RawFd,
        name: &CStr,
        display: &Path,
        dev: u64,
        min_age: Option<Duration>,
        stats: &mut Stats,
    ) {
        let st = match lstat_at(dir, name) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => return stats.fail(display, &e),
        };
        if is_dir(&st) {
            if ids(&st).0 != dev {
                return stats.fail_msg(format!(
                    "Skipped {}: it's on another drive.",
                    display.display()
                ));
            }
            let fd = match open_dir_at(dir, name) {
                Ok(fd) => fd,
                Err(e) => return stats.fail(display, &e),
            };
            // Make sure what we opened is what we looked at (not swapped).
            match fstat(fd.as_raw_fd()) {
                Ok(now) if ids(&now) == ids(&st) => {}
                _ => {
                    return stats.fail_msg(format!(
                        "{} changed while Roomy was working, so it was left alone.",
                        display.display()
                    ))
                }
            }
            match list(fd.as_raw_fd()) {
                Ok(children) => {
                    for c in children {
                        let Ok(cn) = cstr(&c) else { continue };
                        rm_at(fd.as_raw_fd(), &cn, &display.join(&c), dev, min_age, stats);
                    }
                }
                Err(e) => return stats.fail(display, &e),
            }
            drop(fd);
            match unlink_at(dir, name, libc::AT_REMOVEDIR) {
                Ok(()) => stats.removed += 1,
                Err(e)
                    if matches!(e.raw_os_error(), Some(libc::ENOTEMPTY) | Some(libc::EEXIST)) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => stats.fail(display, &e),
            }
            return;
        }
        let is_symlink = st.st_mode & libc::S_IFMT == libc::S_IFLNK;
        if !is_symlink {
            if is_protected_name(&String::from_utf8_lossy(name.to_bytes())) {
                return stats.fail_msg(format!("Kept {}: it's a system file.", display.display()));
            }
            if !older_than_secs(mtime(&st), min_age) {
                return;
            }
        }
        // Files and links (a link is removed as a link).
        match unlink_at(dir, name, 0) {
            Ok(()) => stats.removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => stats.fail(display, &e),
        }
    }

    pub fn remove_path(path: &Path, expected: &Path, min_age: Option<Duration>, stats: &mut Stats) {
        let (Some(parent), Some(name)) = (expected.parent(), path.file_name()) else {
            return stats.fail_msg(format!("Couldn't confirm where {} is.", path.display()));
        };
        let Ok(want) = fs::metadata(parent) else {
            return stats.fail_msg(format!("Couldn't confirm where {} is.", path.display()));
        };
        // Pin the (already link-free, canonical) parent and check it's the
        // same folder that was checked.
        let pfd = match open_dir(parent, false) {
            Ok(fd) => fd,
            Err(e) => return stats.fail(path, &e),
        };
        let pst = match fstat(pfd.as_raw_fd()) {
            Ok(s) => s,
            Err(e) => return stats.fail(path, &e),
        };
        if ids(&pst) != (want.dev(), want.ino()) {
            return stats.fail_msg(format!(
                "{} changed while Roomy was working, so it was left alone.",
                path.display()
            ));
        }
        let Ok(cn) = cstr(name) else { return };
        rm_at(pfd.as_raw_fd(), &cn, path, ids(&pst).0, min_age, stats);
    }

    pub fn remove_children(
        dir: &Path,
        min_age: Option<Duration>,
        stats: &mut Stats,
        check: &mut Check<'_>,
    ) -> io::Result<()> {
        let fd = open_dir(dir, false)?;
        let dev = ids(&fstat(fd.as_raw_fd())?).0;
        for c in list(fd.as_raw_fd())? {
            let child = dir.join(&c);
            if let Err(reason) = check(&child) {
                stats.fail_msg(reason);
                continue;
            }
            let cn = cstr(&c)?;
            rm_at(fd.as_raw_fd(), &cn, &child, dev, min_age, stats);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) {
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "creating a junction failed");
    }

    #[cfg(unix)]
    fn make_dir_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    fn open_guard() -> Guard {
        Guard::default()
    }

    #[test]
    fn permanent_delete_removes_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("victim");
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/f.txt"), b"hello").unwrap();
        fs::write(d.join("g.txt"), b"x").unwrap();
        let stats = delete_path(&open_guard(), &d, Mode::Permanent).unwrap();
        assert!(!d.exists());
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.removed, 5);
    }

    #[test]
    fn permanent_delete_never_follows_links() {
        let outside = tempfile::tempdir().unwrap();
        let precious = outside.path().join("precious.txt");
        fs::write(&precious, b"keep me").unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("victim");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("junk.txt"), b"junk").unwrap();
        make_dir_link(outside.path(), &d.join("link_out"));

        delete_path(&open_guard(), &d, Mode::Permanent).unwrap();
        assert!(!d.exists(), "the folder (and the link in it) is gone");
        assert!(precious.exists(), "the link target must be untouched");
        assert_eq!(fs::read(&precious).unwrap(), b"keep me");
    }

    #[test]
    fn deleting_a_link_removes_only_the_link() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("precious.txt"), b"keep").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("link");
        make_dir_link(outside.path(), &link);
        // Trash can't hold a link recoverably: refused, nothing removed.
        assert!(delete_path(&open_guard(), &link, Mode::Trash).is_err());
        assert!(
            fs::symlink_metadata(&link).is_ok(),
            "link kept in trash mode"
        );
        // Permanent removes just the link.
        delete_path(&open_guard(), &link, Mode::Permanent).unwrap();
        assert!(fs::symlink_metadata(&link).is_err(), "link removed");
        assert!(outside.path().join("precious.txt").exists(), "target kept");
    }

    #[test]
    fn delete_contents_skips_links_and_keeps_folder() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("precious.txt"), b"keep").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        fs::create_dir_all(cache.join("sub")).unwrap();
        fs::write(cache.join("sub/blob"), vec![0u8; 1000]).unwrap();
        make_dir_link(outside.path(), &cache.join("link"));
        let stats = delete_contents(&open_guard(), &cache, None).unwrap();
        assert_eq!(stats.failed, 0);
        assert!(cache.exists());
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        assert!(outside.path().join("precious.txt").exists());
    }

    #[test]
    fn delete_contents_refuses_a_linked_folder() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("precious.txt"), b"keep").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("cache");
        make_dir_link(outside.path(), &link);
        assert!(delete_contents(&open_guard(), &link, None).is_err());
        assert!(outside.path().join("precious.txt").exists());
    }

    #[test]
    fn delete_contents_respects_min_age() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("fresh.tmp"), b"new").unwrap();
        delete_contents(&open_guard(), tmp.path(), Some(Duration::from_secs(3600))).unwrap();
        assert!(tmp.path().join("fresh.tmp").exists());
    }

    #[test]
    fn blocked_paths_are_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let protected = tmp.path().join("system");
        fs::create_dir(&protected).unwrap();
        fs::write(protected.join("vital.dll"), b"x").unwrap();
        let guard = Guard::new(vec![], vec![PathBuf::from(&protected)], vec![]);
        for mode in [Mode::Trash, Mode::Permanent] {
            assert!(delete_path(&guard, &protected, mode).is_err());
            assert!(delete_path(&guard, &protected.join("vital.dll"), mode).is_err());
            // Deleting a parent of a protected tree is blocked too.
            assert!(delete_path(&guard, tmp.path(), mode).is_err());
        }
        assert!(protected.join("vital.dll").exists());
    }

    #[test]
    fn read_only_files_are_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("ro.txt");
        fs::write(&f, b"x").unwrap();
        let mut p = fs::metadata(&f).unwrap().permissions();
        p.set_readonly(true);
        fs::set_permissions(&f, p).unwrap();
        let stats = delete_path(&open_guard(), &f, Mode::Permanent).unwrap();
        assert_eq!(stats.failed, 0);
        assert!(!f.exists());
    }

    #[test]
    fn trash_moves_item_out_of_place() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp
            .path()
            .join(format!("roomy-trash-test-{}.txt", std::process::id()));
        fs::write(&f, b"recycle me").unwrap();
        match delete_path(&open_guard(), &f, Mode::Trash) {
            Ok(stats) => {
                assert_eq!(stats.removed, 1);
                assert!(!f.exists(), "file should have moved to the trash");
            }
            // Headless Linux CI machines may have no trash; never fall back to
            // a permanent delete in that case.
            Err(e) => {
                if !cfg!(target_os = "linux") {
                    panic!("trash failed: {e}");
                }
                assert!(f.exists());
            }
        }
    }

    #[test]
    fn targets_reached_through_a_linked_parent_are_refused() {
        // tmp/real/cache/blob is ordinary, unprotected data. Reaching it via
        // tmp/link (a junction/symlink to tmp/real) must not be allowed.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir_all(real.join("cache")).unwrap();
        fs::write(real.join("cache/blob"), b"keep").unwrap();
        let link = tmp.path().join("link");
        make_dir_link(&real, &link);
        let g = open_guard();
        for mode in [Mode::Trash, Mode::Permanent] {
            assert!(delete_path(&g, &link.join("cache").join("blob"), mode).is_err());
            assert!(delete_path(&g, &link.join("cache"), mode).is_err());
        }
        assert!(delete_contents(&g, &link.join("cache"), None).is_err());
        assert!(
            real.join("cache/blob").exists(),
            "nothing behind the link was touched"
        );
        // The same file at its real location is fine.
        delete_path(&g, &real.join("cache").join("blob"), Mode::Permanent).unwrap();
        assert!(!real.join("cache/blob").exists());
    }

    #[test]
    fn folders_holding_a_registry_hive_are_refused_in_both_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("someone");
        fs::create_dir_all(profile.join("AppData")).unwrap();
        fs::write(profile.join("NTUSER.DAT"), b"hive").unwrap();
        fs::write(profile.join("AppData").join("UsrClass.dat"), b"hive").unwrap();
        fs::write(profile.join("notes.txt"), b"x").unwrap();
        let g = open_guard();
        for mode in [Mode::Trash, Mode::Permanent] {
            let err = delete_path(&g, &profile, mode).unwrap_err();
            assert!(err.contains(".DAT") || err.contains(".dat"), "{err}");
        }
        // Clearing contents skips the protected file and keeps it.
        let stats = delete_contents(&g, &profile, None).unwrap();
        assert!(stats.failed >= 1);
        assert!(profile.join("NTUSER.DAT").exists());
        assert!(profile.join("AppData").join("UsrClass.dat").exists());
        assert!(!profile.join("notes.txt").exists());
    }

    #[test]
    fn permanent_recursion_keeps_protected_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("d");
        fs::create_dir_all(d.join("deep")).unwrap();
        fs::write(d.join("deep").join("pagefile.sys"), b"x").unwrap();
        let mut stats = Stats::default();
        remove_tree(&d, None, &mut stats);
        assert!(d.join("deep").join("pagefile.sys").exists());
        assert!(stats.failed >= 1);
    }

    #[test]
    fn removal_refuses_a_target_that_is_not_where_it_was_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        let other = tmp.path().join("other");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(real.join("f"), b"x").unwrap();
        // Pretend the check resolved somewhere else: the pinned handle won't match.
        let mut stats = Stats::default();
        sys::remove_path(&real, &other.join("real"), None, &mut stats);
        assert!(real.join("f").exists(), "nothing removed");
        assert_eq!(stats.failed, 1);
    }

    #[test]
    fn mount_points_are_detected() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_mount_point(tmp.path()));
        #[cfg(windows)]
        assert!(is_mount_point(Path::new(&format!(
            "{}\\",
            std::env::var("SystemDrive").unwrap_or("C:".into())
        ))));
    }

    #[test]
    fn missing_path_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let stats = delete_path(&open_guard(), &tmp.path().join("gone"), Mode::Permanent).unwrap();
        assert_eq!(stats.removed, 0);
    }
}
