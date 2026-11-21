//! Volume listing, disk usage, elevation and Recycle Bin helpers.

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// Mount point, e.g. `C:\` or `/`.
    pub path: String,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    /// "fixed", "removable", "network" or "other".
    pub kind: String,
    /// True for the volume the OS runs from.
    pub is_system: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiskUsage {
    pub total: u64,
    pub free: u64,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::scan::platform::{to_wide, volume_root};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;

    fn wide_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    pub fn disk_usage(path: &Path) -> Option<DiskUsage> {
        let root = volume_root(path).unwrap_or_else(|| path.to_string_lossy().into_owned());
        let w = to_wide(&root);
        let mut avail = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
        (ok != 0).then_some(DiskUsage { total, free })
    }

    pub fn list_volumes() -> Vec<Volume> {
        let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{letter}:\\");
            let w = to_wide(&root);
            let dtype = unsafe { GetDriveTypeW(w.as_ptr()) };
            let kind = match dtype {
                // GetDriveTypeW: 2 removable, 3 fixed, 4 remote, 6 RAM disk;
                // 0 unknown, 1 no root dir, 5 CD-ROM are skipped.
                3 => "fixed",
                2 => "removable",
                4 => "network",
                0 | 1 | 5 => continue,
                _ => "other",
            };
            let Some(usage) = disk_usage(Path::new(&root)) else {
                continue;
            };
            let mut label = vec![0u16; 261];
            let mut fs = vec![0u16; 64];
            unsafe {
                GetVolumeInformationW(
                    w.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs.as_mut_ptr(),
                    fs.len() as u32,
                );
            }
            out.push(Volume {
                path: root.clone(),
                label: wide_to_string(&label),
                fs: wide_to_string(&fs),
                total: usage.total,
                free: usage.free,
                kind: kind.into(),
                is_system: root[..2].eq_ignore_ascii_case(&system_drive),
            });
        }
        out
    }

    pub fn is_elevated() -> bool {
        use windows_sys::Win32::Security::*;
        use windows_sys::Win32::System::Threading::*;
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                &mut elev as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            );
            CloseHandle(token);
            ok != 0 && elev.TokenIsElevated != 0
        }
    }

    /// When elevated, enable SeBackupPrivilege so directories with
    /// restrictive ACLs can still be listed (read-only) during scans.
    pub fn enable_backup_privilege() {
        use windows_sys::Win32::Security::*;
        use windows_sys::Win32::System::Threading::*;
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            ) == 0
            {
                return;
            }
            let name = to_wide("SeBackupPrivilege");
            let mut luid = std::mem::zeroed();
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) != 0 {
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES {
                        Luid: luid,
                        Attributes: SE_PRIVILEGE_ENABLED,
                    }],
                };
                AdjustTokenPrivileges(token, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut());
            }
            CloseHandle(token);
        }
    }

    /// Restart Roomy with administrator rights (shows the UAC prompt).
    pub fn relaunch_elevated() -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let verb = to_wide("runas");
        let file = to_wide(&exe.to_string_lossy());
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if (r as isize) > 32 {
            Ok(())
        } else {
            Err(
                "Windows didn't start Roomy as administrator (the prompt may have been declined)."
                    .into(),
            )
        }
    }

    /// Size of the NTFS master file table, if Windows lets us ask.
    pub fn ntfs_mft_bytes(path: &Path) -> Option<u64> {
        use windows_sys::Win32::System::Ioctl::{
            FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER,
        };
        use windows_sys::Win32::System::IO::DeviceIoControl;
        let root = volume_root(path)?;
        let letter = root.trim_end_matches('\\');
        if letter.len() != 2 {
            return None;
        }
        let dev = to_wide(&format!(r"\\.\{letter}"));
        unsafe {
            let h = CreateFileW(
                dev.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut data: NTFS_VOLUME_DATA_BUFFER = std::mem::zeroed();
            let mut ret = 0u32;
            let ok = DeviceIoControl(
                h,
                FSCTL_GET_NTFS_VOLUME_DATA,
                std::ptr::null(),
                0,
                &mut data as *mut _ as *mut _,
                std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
                &mut ret,
                std::ptr::null_mut(),
            );
            CloseHandle(h);
            (ok != 0).then_some(data.MftValidDataLength.max(0) as u64)
        }
    }

    pub fn recycle_bin_size() -> Option<(u64, u64)> {
        use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            i64Size: 0,
            i64NumItems: 0,
        };
        let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
        (hr >= 0).then_some((info.i64Size.max(0) as u64, info.i64NumItems.max(0) as u64))
    }

    pub fn empty_recycle_bin() -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::{
            SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND,
        };
        let hr = unsafe {
            SHEmptyRecycleBinW(
                std::ptr::null_mut(),
                std::ptr::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        };
        // S_OK, or E_UNEXPECTED when the bin is already empty.
        if hr >= 0 || hr == 0x8000_FFFFu32 as i32 {
            Ok(())
        } else {
            Err(format!(
                "Windows couldn't empty the Recycle Bin (0x{hr:08X})."
            ))
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;

    pub fn disk_usage(path: &Path) -> Option<DiskUsage> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let frsize = st.f_frsize as u64;
        Some(DiskUsage {
            total: st.f_blocks as u64 * frsize,
            free: st.f_bfree as u64 * frsize,
        })
    }

    fn mount_points() -> Vec<(String, String)> {
        let mut v = Vec::new();
        #[cfg(target_os = "linux")]
        {
            const REAL: &[&str] = &[
                "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "vfat", "exfat", "ntfs", "ntfs3",
                "f2fs", "fuseblk", "bcachefs",
            ];
            if let Ok(s) = std::fs::read_to_string("/proc/mounts") {
                for line in s.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 && REAL.contains(&parts[2]) {
                        let mp = parts[1].replace("\\040", " ");
                        if !mp.starts_with("/boot") && !mp.starts_with("/snap") {
                            v.push((mp, parts[2].to_string()));
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            // The writable data volume holds user data on modern macOS.
            if Path::new("/System/Volumes/Data").exists() {
                v.push(("/System/Volumes/Data".into(), "apfs".into()));
            } else {
                v.push(("/".into(), "apfs".into()));
            }
            if let Ok(rd) = std::fs::read_dir("/Volumes") {
                for e in rd.flatten() {
                    let p = e.path();
                    if let Ok(md) = std::fs::symlink_metadata(&p) {
                        if md.file_type().is_symlink() {
                            continue; // "Macintosh HD" -> /
                        }
                    }
                    v.push((p.to_string_lossy().into_owned(), String::new()));
                }
            }
        }
        if v.is_empty() {
            v.push(("/".into(), String::new()));
        }
        v
    }

    pub fn list_volumes() -> Vec<Volume> {
        mount_points()
            .into_iter()
            .filter_map(|(mp, fs)| {
                let u = disk_usage(Path::new(&mp))?;
                if u.total == 0 {
                    return None;
                }
                let is_system = mp == "/" || mp == "/System/Volumes/Data";
                let label = if is_system {
                    if cfg!(target_os = "macos") {
                        "Macintosh HD".into()
                    } else {
                        "System".into()
                    }
                } else {
                    Path::new(&mp)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| mp.clone())
                };
                Some(Volume {
                    path: mp,
                    label,
                    fs,
                    total: u.total,
                    free: u.free,
                    kind: "fixed".into(),
                    is_system,
                })
            })
            .collect()
    }

    pub fn is_elevated() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    pub fn enable_backup_privilege() {}

    pub fn relaunch_elevated() -> Result<(), String> {
        Err("Running as administrator is only available on Windows.".into())
    }

    pub fn ntfs_mft_bytes(_path: &Path) -> Option<u64> {
        None
    }

    pub fn recycle_bin_size() -> Option<(u64, u64)> {
        let trash = dirs::home_dir()?.join(".Trash");
        if !trash.exists() {
            return None;
        }
        let (bytes, files) = crate::scan::measure(
            &trash,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        Some((bytes, files))
    }

    pub fn empty_recycle_bin() -> Result<(), String> {
        Err("Empty the Trash from Finder.".into())
    }
}

pub use imp::*;

/// Mount point of the volume containing `path`, if `path` *is* a mount point.
pub fn is_volume_root(path: &Path) -> bool {
    list_volumes().iter().any(|v| {
        let a = v.path.trim_end_matches(['\\', '/']);
        let b = path.to_string_lossy();
        let b = b.trim_end_matches(['\\', '/']);
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b || (v.path == "/" && b.is_empty())
        }
    })
}
