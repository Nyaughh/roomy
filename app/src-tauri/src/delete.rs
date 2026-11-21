//! Deleting files and folders: Recycle Bin/Trash by default, permanent only
//! when asked. Recursive deletes never follow junctions or symlinks: a link
//! is removed as a link and whatever it points at is left untouched.

use crate::safety::{is_protected_name, linked_ancestor, Guard};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
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

#[cfg(windows)]
fn link_is_dir(md: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes() & 0x10 != 0 // FILE_ATTRIBUTE_DIRECTORY
}

#[cfg(not(windows))]
fn link_is_dir(_md: &fs::Metadata) -> bool {
    false
}

/// Remove a link itself, never its target.
fn remove_link(path: &Path, md: &fs::Metadata) -> io::Result<()> {
    if link_is_dir(md) {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

fn remove_file_forced(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            // Read-only files can't be deleted on Windows until the flag is cleared.
            let md = fs::symlink_metadata(path)?;
            let mut perms = md.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                fs::set_permissions(path, perms)?;
                fs::remove_file(path)
            } else {
                Err(e)
            }
        }
        r => r,
    }
}

fn older_than(md: &fs::Metadata, min_age: Option<Duration>) -> bool {
    match min_age {
        None => true,
        Some(age) => md
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .map(|d| d >= age)
            .unwrap_or(false),
    }
}

/// Permanently remove `path`, recursing into real directories only. Items
/// that can't be removed (in use, no permission) are skipped and counted.
pub fn remove_tree(path: &Path, min_age: Option<Duration>, stats: &mut Stats) {
    let dev = device_of(path);
    remove_tree_on(path, min_age, stats, dev);
}

#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> Option<u64> {
    None
}

/// True if `md` (a directory) lives on a different volume than `dev`.
#[cfg(unix)]
fn other_volume(md: &fs::Metadata, dev: Option<u64>) -> bool {
    use std::os::unix::fs::MetadataExt;
    dev.map(|d| md.dev() != d).unwrap_or(false)
}

#[cfg(not(unix))]
fn other_volume(_md: &fs::Metadata, _dev: Option<u64>) -> bool {
    false
}

/// A Windows volume mount point (a drive mounted into a folder).
fn is_volume_mount(path: &Path) -> bool {
    cfg!(windows)
        && fs::read_link(path)
            .map(|t| t.to_string_lossy().contains("Volume{"))
            .unwrap_or(false)
}

/// Walk a folder (never following links) and refuse if it holds something
/// that must never be deleted: a registry hive or system file, or another
/// drive mounted inside it. Run before either kind of delete.
pub fn inspect(dir: &Path) -> Result<(), String> {
    let dev = device_of(dir);
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
            if is_link(&md) {
                if is_volume_mount(&p) {
                    return Err(format!(
                        "Another drive is mounted inside it at {}.",
                        p.display()
                    ));
                }
                continue; // a link is removed as a link, never followed
            }
            if md.is_dir() {
                if other_volume(&md, dev) {
                    return Err(format!(
                        "Another drive is mounted inside it at {}.",
                        p.display()
                    ));
                }
                stack.push(p);
            }
        }
    }
    Ok(())
}

fn remove_tree_on(path: &Path, min_age: Option<Duration>, stats: &mut Stats, dev: Option<u64>) {
    let md = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => return stats.fail(path, &e),
    };
    if is_link(&md) && is_volume_mount(path) {
        stats.failed += 1;
        stats.first_error.get_or_insert_with(|| {
            format!(
                "Skipped {}: another drive is mounted there.",
                path.display()
            )
        });
        return;
    }
    if is_link(&md) {
        match remove_link(path, &md) {
            Ok(()) => stats.removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => stats.fail(path, &e),
        }
        return;
    }
    if md.is_dir() {
        if other_volume(&md, dev) {
            stats.failed += 1;
            stats.first_error.get_or_insert_with(|| {
                format!("Skipped {}: it's on another drive.", path.display())
            });
            return;
        }
        match fs::read_dir(path) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    remove_tree_on(&entry.path(), min_age, stats, dev);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => return stats.fail(path, &e),
        }
        match fs::remove_dir(path) {
            Ok(()) => stats.removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                // Not empty because something inside was skipped: that was
                // already counted, so don't count the folder again.
                let not_empty = fs::read_dir(path)
                    .map(|mut r| r.next().is_some())
                    .unwrap_or(false);
                if !not_empty {
                    stats.fail(path, &e);
                }
            }
        }
        return;
    }
    if !older_than(&md, min_age) {
        return;
    }
    if path
        .file_name()
        .map(|n| is_protected_name(&n.to_string_lossy()))
        .unwrap_or(false)
    {
        stats.failed += 1;
        stats
            .first_error
            .get_or_insert_with(|| format!("Kept {}: it's a system file.", path.display()));
        return;
    }
    match remove_file_forced(path) {
        Ok(()) => stats.removed += 1,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => stats.fail(path, &e),
    }
}

/// Delete one item after checking it against the guard.
pub fn delete_path(guard: &Guard, path: &Path, mode: Mode) -> Result<Stats, String> {
    guard.check(path)?;
    let md = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Stats::default()),
        Err(e) => return Err(describe(path, &e)),
    };
    if md.is_dir() && !is_link(&md) {
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
            if is_link(&md) {
                // Recycling a link can confuse the shell; a link holds no data,
                // so removing it outright loses nothing.
                remove_link(path, &md).map_err(|e| describe(path, &e))?;
                return Ok(Stats {
                    removed: 1,
                    ..Default::default()
                });
            }
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
    if !md.is_dir() {
        let mut stats = Stats::default();
        guard.check(dir)?;
        remove_tree(dir, min_age, &mut stats);
        return Ok(stats);
    }
    let mut stats = Stats::default();
    let rd = fs::read_dir(dir).map_err(|e| describe(dir, &e))?;
    for entry in rd.flatten() {
        let p = entry.path();
        let checked = guard.check(&p).and_then(|_| {
            let is_real_dir = fs::symlink_metadata(&p)
                .map(|m| m.is_dir() && !is_link(&m))
                .unwrap_or(false);
            if is_real_dir {
                inspect(&p)
            } else {
                Ok(())
            }
        });
        if let Err(reason) = checked {
            stats.failed += 1;
            stats.first_error.get_or_insert(reason);
            continue;
        }
        remove_tree(&p, min_age, &mut stats);
    }
    Ok(stats)
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
        for mode in [Mode::Permanent, Mode::Trash] {
            if !link.exists() {
                make_dir_link(outside.path(), &link);
            }
            delete_path(&open_guard(), &link, mode).unwrap();
            assert!(
                fs::symlink_metadata(&link).is_err(),
                "link removed ({mode:?})"
            );
            assert!(
                outside.path().join("precious.txt").exists(),
                "target kept ({mode:?})"
            );
        }
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
    fn missing_path_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let stats = delete_path(&open_guard(), &tmp.path().join("gone"), Mode::Permanent).unwrap();
        assert_eq!(stats.removed, 0);
    }
}
