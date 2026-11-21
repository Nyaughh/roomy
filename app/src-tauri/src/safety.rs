//! Deletion guard: hard-blocks removing paths that would break the OS, apps
//! or the user's profile.
//!
//! Three kinds of rules:
//! * **Protected paths** may not be deleted themselves, and neither may any
//!   of their ancestors (deleting `C:\Users` would delete a profile). Their
//!   contents are fine to delete.
//! * **Protected trees** may not be touched at all (e.g. the Windows folder),
//!   except for explicitly allowed sub-paths such as `Windows\Temp`.
//! * **Protected names** are files that must never be removed wherever they
//!   live (registry hives, page/hibernation files).

use std::path::{Component, Path, PathBuf};

const UNVERIFIED: &str = "Roomy couldn't confirm where this is on disk, so it left it alone.";

#[derive(Debug, Clone, Default)]
pub struct Guard {
    protected: Vec<PathBuf>,
    trees: Vec<PathBuf>,
    allowed: Vec<PathBuf>,
}

const PROTECTED_NAMES: &[&str] = &[
    "ntuser.dat",
    "ntuser.ini",
    "ntuser.pol",
    "usrclass.dat",
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "bootmgr",
    "bootnxt",
];

/// Normalise a path into lowercase (on Windows) components for comparison.
fn key(p: &Path) -> Vec<String> {
    let s = p.to_string_lossy();
    let s = s
        .strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(&s).to_string());
    let p = PathBuf::from(if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s
    });
    let mut out = Vec::new();
    for c in p.components() {
        match c {
            Component::Prefix(px) => out.push(px.as_os_str().to_string_lossy().into_owned()),
            Component::RootDir => out.push(String::from("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n.to_string_lossy().into_owned()),
        }
    }
    if cfg!(windows) {
        for c in &mut out {
            *c = c.to_lowercase();
        }
    } else if out.len() >= 4 && out[1] == "System" && out[2] == "Volumes" && out[3] == "Data" {
        // macOS: the writable data volume is firmlinked into "/", so
        // /System/Volumes/Data/Users/... is the same place as /Users/...
        out.drain(1..4);
    }
    out
}

/// True for files that must never be deleted wherever they are
/// (registry hives, page/hibernation files).
pub fn is_protected_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    PROTECTED_NAMES
        .iter()
        .any(|p| lower == *p || (lower.starts_with(p) && p.ends_with(".dat")))
}

/// Links that are part of the operating system's own layout (macOS /var and
/// /tmp, or a link on the way to the user's home or temp folder). Paths
/// through these are fine; any other link in a path is not.
fn os_link_aliases() -> &'static Vec<Vec<String>> {
    static ALIASES: std::sync::OnceLock<Vec<Vec<String>>> = std::sync::OnceLock::new();
    ALIASES.get_or_init(|| {
        let mut v: Vec<Vec<String>> = Vec::new();
        if cfg!(target_os = "macos") {
            for p in ["/var", "/tmp", "/etc"] {
                v.push(key(Path::new(p)));
            }
        }
        for base in [dirs::home_dir(), Some(std::env::temp_dir())]
            .into_iter()
            .flatten()
        {
            for a in base.ancestors() {
                if std::fs::symlink_metadata(a)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    v.push(key(a));
                }
            }
        }
        v
    })
}

/// The first folder above `path` that is a symlink or junction (other than
/// the OS's own aliases), if any.
pub fn linked_ancestor(path: &Path) -> Option<PathBuf> {
    for a in path.ancestors().skip(1) {
        if a.parent().is_none() || a.as_os_str().is_empty() {
            break;
        }
        let is_link = std::fs::symlink_metadata(a)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_link && !os_link_aliases().contains(&key(a)) {
            return Some(a.to_path_buf());
        }
    }
    None
}

fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == prefix[..]
}

impl Guard {
    pub fn new(protected: Vec<PathBuf>, trees: Vec<PathBuf>, allowed: Vec<PathBuf>) -> Self {
        Guard {
            protected,
            trees,
            allowed,
        }
    }

    /// Guard configured for the current machine.
    pub fn system() -> Self {
        let mut protected: Vec<PathBuf> = Vec::new();
        let mut trees: Vec<PathBuf> = Vec::new();
        let mut allowed: Vec<PathBuf> = Vec::new();
        let home = dirs::home_dir();

        if let Some(h) = &home {
            protected.push(h.clone());
            if let Some(parent) = h.parent() {
                protected.push(parent.to_path_buf()); // C:\Users or /Users or /home
                                                      // Every account's profile, not just this one.
                if let Ok(rd) = std::fs::read_dir(parent) {
                    for e in rd.flatten() {
                        protected.push(e.path());
                    }
                }
            }
        }
        for d in [
            dirs::desktop_dir(),
            dirs::document_dir(),
            dirs::download_dir(),
            dirs::picture_dir(),
            dirs::audio_dir(),
            dirs::video_dir(),
            dirs::config_dir(),
            dirs::data_local_dir(),
            dirs::cache_dir(),
        ]
        .into_iter()
        .flatten()
        {
            protected.push(d);
        }
        // Never let Roomy delete itself.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                trees.push(dir.to_path_buf());
            }
        }

        #[cfg(windows)]
        {
            let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
            let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
            let drive = PathBuf::from(format!("{sys_drive}\\"));
            let windir = env("SystemRoot").unwrap_or_else(|| drive.join("Windows"));
            trees.push(windir.clone());
            for t in [
                "System Volume Information",
                "$Recycle.Bin",
                "Recovery",
                "Boot",
                "EFI",
                "$WinREAgent",
                "$SysReset",
                "$Windows.~BT",
                "$Windows.~WS",
                "Config.Msi",
                "Documents and Settings",
            ] {
                trees.push(drive.join(t));
            }
            for k in [
                "ProgramFiles",
                "ProgramFiles(x86)",
                "ProgramW6432",
                "ProgramData",
            ] {
                if let Some(p) = env(k) {
                    protected.push(p);
                }
            }
            if let Some(pf) = env("ProgramFiles") {
                trees.push(pf.join("WindowsApps"));
                trees.push(pf.join("Windows Defender"));
                trees.push(pf.join("Common Files"));
                trees.push(pf.join("Windows NT"));
            }
            if let Some(pf) = env("ProgramFiles(x86)") {
                trees.push(pf.join("Common Files"));
            }
            if let Some(pd) = env("ProgramData") {
                trees.push(pd.join("Microsoft"));
                trees.push(pd.join("Package Cache"));
                allowed.push(pd.join(r"Microsoft\Windows\WER\ReportArchive"));
                allowed.push(pd.join(r"Microsoft\Windows\WER\ReportQueue"));
            }
            if let Some(pub_) = env("PUBLIC") {
                protected.push(pub_);
            }
            if let Some(h) = &home {
                protected.push(h.join("AppData"));
                protected.push(h.join(r"AppData\LocalLow"));
                protected.push(h.join("OneDrive"));
                trees.push(h.join(r"AppData\Local\Microsoft\WindowsApps"));
                trees.push(h.join(r"AppData\Local\Packages"));
                trees.push(h.join(r"AppData\Roaming\Microsoft\Windows\Start Menu"));
                trees.push(h.join(r"AppData\Roaming\Microsoft\Protect"));
                trees.push(h.join(r"AppData\Roaming\Microsoft\Crypto"));
                trees.push(h.join(r"AppData\Roaming\Microsoft\SystemCertificates"));
                trees.push(h.join(r"AppData\Local\Microsoft\Credentials"));
                trees.push(h.join(r"AppData\Roaming\Microsoft\Credentials"));
                trees.push(h.join(r"AppData\Local\Microsoft\Vault"));
            }
            for a in [
                "Temp",
                "Minidump",
                "MEMORY.DMP",
                "LiveKernelReports",
                r"SoftwareDistribution\Download",
                r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache",
            ] {
                allowed.push(windir.join(a));
            }
            // Every drive root is protected.
            for l in b'A'..=b'Z' {
                protected.push(PathBuf::from(format!("{}:\\", l as char)));
            }
        }

        #[cfg(unix)]
        {
            protected.push(PathBuf::from("/"));
            for p in [
                "/Applications",
                "/Users",
                "/home",
                "/opt",
                "/Volumes",
                "/mnt",
                "/media",
                "/srv",
                "/root",
                "/snap",
            ] {
                protected.push(PathBuf::from(p));
            }
            for t in [
                "/System",
                "/Library",
                "/usr",
                "/bin",
                "/sbin",
                "/etc",
                "/var",
                "/private",
                "/dev",
                "/boot",
                "/lib",
                "/lib32",
                "/lib64",
                "/libx32",
                "/proc",
                "/sys",
                "/run",
                "/cores",
                "/Applications/Utilities",
            ] {
                trees.push(PathBuf::from(t));
            }
            allowed.push(PathBuf::from("/Library/Caches"));
            allowed.push(PathBuf::from("/cores"));
            if let Some(h) = &home {
                protected.push(h.join("Library"));
                protected.push(h.join("Library/Application Support"));
                protected.push(h.join("Library/Mobile Documents"));
                trees.push(h.join("Library/Keychains"));
                trees.push(h.join("Library/Preferences"));
                trees.push(h.join(".ssh"));
                trees.push(h.join(".gnupg"));
            }
        }
        // `/cores` is both a tree and allowed: the allow wins, by design.
        Guard::new(protected, trees, allowed)
    }

    /// Ok if `path` may be deleted, otherwise a plain-language reason.
    pub fn check(&self, path: &Path) -> Result<(), String> {
        self.check_lexical(path)?;
        let md = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            // Nothing there: nothing will be deleted.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(UNVERIFIED.into()),
        };
        // Only delete things at their real location: a link anywhere above
        // the target could point outside what the user reviewed.
        if let Some(link) = linked_ancestor(path) {
            return Err(format!(
                "This is reached through a link ({}). Roomy only deletes things at their real location.",
                link.display()
            ));
        }
        // Check where the path really is. If that can't be confirmed, refuse.
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(UNVERIFIED.into());
        };
        let real_parent = std::fs::canonicalize(parent).map_err(|_| UNVERIFIED.to_string())?;
        self.check_lexical(&real_parent.join(name))?;
        if !md.file_type().is_symlink() {
            // (Deleting a link removes only the link, so its target doesn't matter.)
            let real = std::fs::canonicalize(path).map_err(|_| UNVERIFIED.to_string())?;
            self.check_lexical(&real)?;
        }
        Ok(())
    }

    fn check_lexical(&self, path: &Path) -> Result<(), String> {
        let k = key(path);
        if !path.is_absolute() && !path.to_string_lossy().starts_with(r"\\") {
            return Err("Roomy only deletes items by their full path.".into());
        }
        // A bare root (`C:\`, `/`) or a prefix alone.
        let is_root = k.len() <= 2 && k.last().map(|c| c == "/").unwrap_or(true);
        if is_root {
            return Err("That's the top of a drive. Roomy never deletes a whole drive.".into());
        }
        if let Some(name) = k.last() {
            if is_protected_name(name) {
                return Err(format!(
                    "“{name}” is a system file that Windows or your account needs."
                ));
            }
        }
        let allowed = self.allowed.iter().any(|a| starts_with(&k, &key(a)));
        for t in &self.trees {
            let tk = key(t);
            if starts_with(&k, &tk) && !allowed {
                return Err(format!(
                    "Inside {}, which your system needs to run. Roomy won't touch it.",
                    t.display()
                ));
            }
            if starts_with(&tk, &k) {
                return Err(format!(
                    "This contains {}, which your system needs to run.",
                    t.display()
                ));
            }
        }
        for p in &self.protected {
            let pk = key(p);
            if starts_with(&pk, &k) {
                return Err(if pk == k {
                    format!(
                        "{} is an essential folder. You can delete things inside it, but not the folder itself.",
                        p.display()
                    )
                } else {
                    format!("This contains {}, an essential folder.", p.display())
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn guard() -> Guard {
        Guard::new(
            vec![
                PathBuf::from(r"C:\"),
                PathBuf::from(r"C:\Users"),
                PathBuf::from(r"C:\Users\ann"),
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Users\ann\AppData\Local"),
            ],
            vec![PathBuf::from(r"C:\Windows")],
            vec![PathBuf::from(r"C:\Windows\Temp")],
        )
    }

    #[cfg(windows)]
    #[test]
    fn blocks_critical_windows_paths() {
        let g = guard();
        for p in [
            r"C:\",
            r"C:\Windows",
            r"C:\Windows\System32",
            r"c:\windows\system32\drivers\etc\hosts",
            r"C:\Program Files",
            r"C:\Users",
            r"C:\Users\ann",
            r"C:\USERS\ANN",
            r"C:\Users\ann\AppData\Local",
            r"\\?\C:\Windows\explorer.exe",
            r"C:\Users\ann\NTUSER.DAT",
            r"C:\Users\ann\ntuser.dat.LOG1",
            r"C:\Users\ann\AppData\Local\Microsoft\Windows\UsrClass.dat",
            r"C:\pagefile.sys",
            r"C:\Users\ann\..\..\Windows\notepad.exe",
        ] {
            assert!(
                g.check_lexical(Path::new(p)).is_err(),
                "{p} should be blocked"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn allows_ordinary_and_allowlisted_paths() {
        let g = guard();
        for p in [
            r"C:\Users\ann\Downloads\movie.mkv",
            r"C:\Users\ann\AppData\Local\Temp\x.tmp",
            r"C:\Program Files\SomeApp",
            r"C:\Windows\Temp\setup.log",
            r"C:\Windows\Temp",
            r"D:\Games\old",
        ] {
            assert!(
                g.check_lexical(Path::new(p)).is_ok(),
                "{p} should be allowed"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_into_protected_tree_is_blocked() {
        // A junction inside a harmless folder that points into a protected
        // tree must not make the protected contents deletable.
        let tmp = tempfile::tempdir().unwrap();
        let protected_root = tempfile::tempdir().unwrap();
        std::fs::write(protected_root.path().join("vital.bin"), b"x").unwrap();
        let link = tmp.path().join("innocent");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(protected_root.path())
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok);
        let real = std::fs::canonicalize(protected_root.path()).unwrap();
        let g = Guard::new(vec![], vec![real], vec![]);
        // Going *through* the junction resolves into the protected tree.
        assert!(g.check(&link.join("vital.bin")).is_err());
        // Removing the junction itself only removes a link: allowed.
        assert!(g.check(&link).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn blocks_critical_unix_paths() {
        let g = Guard::new(
            vec![PathBuf::from("/"), PathBuf::from("/Users/ann")],
            vec![PathBuf::from("/System"), PathBuf::from("/usr")],
            vec![],
        );
        for p in [
            "/",
            "/System/Library",
            "/usr/bin/ls",
            "/Users",
            "/Users/ann",
        ] {
            assert!(
                g.check_lexical(Path::new(p)).is_err(),
                "{p} should be blocked"
            );
        }
        for p in ["/Users/ann/Downloads/a.zip", "/tmp/x"] {
            assert!(
                g.check_lexical(Path::new(p)).is_ok(),
                "{p} should be allowed"
            );
        }
    }

    #[test]
    fn relative_paths_are_rejected() {
        let g = Guard::default();
        assert!(g.check_lexical(Path::new("some/relative")).is_err());
    }

    #[test]
    fn system_guard_blocks_home() {
        let g = Guard::system();
        let home = dirs::home_dir().unwrap();
        assert!(g.check(&home).is_err());
        assert!(g.check(home.parent().unwrap()).is_err());
        assert!(g
            .check(&home.join("some-file-that-does-not-exist.txt"))
            .is_ok());
    }

    #[test]
    fn every_profile_on_the_machine_is_protected() {
        let g = Guard::system();
        let home = dirs::home_dir().unwrap();
        let users = home.parent().unwrap();
        for e in std::fs::read_dir(users).unwrap().flatten() {
            let p = e.path();
            assert!(
                g.check_lexical(&p).is_err(),
                "{} should be protected",
                p.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn macos_data_volume_paths_are_normalised() {
        let g = Guard::new(
            vec![PathBuf::from("/"), PathBuf::from("/Users/ann")],
            vec![PathBuf::from("/System"), PathBuf::from("/private")],
            vec![],
        );
        assert!(g
            .check_lexical(Path::new("/System/Volumes/Data/Users/ann/Downloads/x.zip"))
            .is_ok());
        assert!(g
            .check_lexical(Path::new("/System/Volumes/Data/Users/ann"))
            .is_err());
        assert!(g
            .check_lexical(Path::new("/System/Volumes/Data/private/var/x"))
            .is_err());
        assert!(g.check_lexical(Path::new("/System/Volumes/Data")).is_err());
        assert!(g
            .check_lexical(Path::new("/System/Volumes/Preboot/x"))
            .is_err());
    }
}
