//! Duplicate large files: same size, then same head/tail sample, then same
//! full BLAKE3 hash. Hard links are already collapsed by the scanner, so
//! they never show up as duplicates.

use crate::safety::Guard;
use crate::tree::{Tree, FLAG_HARDLINK, KIND_FILE};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const MIN_SIZE: u64 = 10 * 1024 * 1024;
const SAMPLE: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupeFile {
    pub id: u32,
    pub path: String,
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupeGroup {
    pub size: u64,
    /// Bytes freed if all but one copy are removed.
    pub wasted: u64,
    pub files: Vec<DupeFile>,
}

pub struct DupeProgress {
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
}

fn sample_hash(path: &Path, size: u64) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; SAMPLE];
    let n = read_full(&mut f, &mut buf)?;
    h.update(&buf[..n]);
    if size > (SAMPLE * 2) as u64 {
        f.seek(SeekFrom::End(-(SAMPLE as i64))).ok()?;
        let n = read_full(&mut f, &mut buf)?;
        h.update(&buf[..n]);
    }
    Some(*h.finalize().as_bytes())
}

fn read_full(f: &mut File, buf: &mut [u8]) -> Option<usize> {
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(total)
}

fn full_hash(path: &Path, progress: &DupeProgress, cancel: &AtomicBool) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        h.update(&buf[..n]);
        progress.done_bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    Some(*h.finalize().as_bytes())
}

fn group_by<K: std::hash::Hash + Eq + Send, T: Send + Sync>(
    items: Vec<T>,
    key: impl Fn(&T) -> Option<K> + Sync,
) -> Vec<Vec<T>> {
    let keyed: Vec<(Option<K>, T)> = items.into_par_iter().map(|t| (key(&t), t)).collect();
    let mut map: HashMap<K, Vec<T>> = HashMap::new();
    for (k, t) in keyed {
        if let Some(k) = k {
            map.entry(k).or_default().push(t);
        }
    }
    map.into_values().filter(|g| g.len() > 1).collect()
}

/// Candidate files of at least `MIN_SIZE`, outside protected locations.
pub fn candidates(tree: &Tree, guard: &Guard) -> Vec<(u32, PathBuf, u64)> {
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for (i, n) in tree.nodes.iter().enumerate() {
        if n.kind == KIND_FILE
            && !n.is_deleted()
            && n.flags & FLAG_HARDLINK == 0
            && n.logical >= MIN_SIZE
        {
            by_size.entry(n.logical).or_default().push(i as u32);
        }
    }
    let mut out = Vec::new();
    for (size, ids) in by_size {
        if ids.len() < 2 {
            continue;
        }
        let mut keep = Vec::new();
        for id in ids {
            let p = tree.path(id);
            if guard.check(&p).is_ok() {
                keep.push((id, p, size));
            }
        }
        if keep.len() > 1 {
            out.extend(keep);
        }
    }
    out
}

pub fn find(
    tree_files: Vec<(u32, PathBuf, u64)>,
    mtimes: &HashMap<u32, i64>,
    progress: &DupeProgress,
    cancel: &AtomicBool,
) -> Vec<DupeGroup> {
    // 1. same size
    let by_size = group_by(tree_files, |(_, _, s)| Some(*s));
    // 2. same head + tail sample
    let mut stage2 = Vec::new();
    for g in by_size {
        stage2.extend(group_by(g, |(_, p, s)| sample_hash(p, *s)));
    }
    let total: u64 = stage2.iter().flatten().map(|(_, _, s)| *s).sum();
    progress.total_bytes.store(total, Ordering::Relaxed);
    // 3. same full hash
    let mut groups = Vec::new();
    for g in stage2 {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        for set in group_by(g, |(_, p, _)| full_hash(p, progress, cancel)) {
            let size = set[0].2;
            let mut files: Vec<DupeFile> = set
                .into_iter()
                .map(|(id, p, _)| DupeFile {
                    id,
                    path: p.to_string_lossy().into_owned(),
                    mtime: mtimes.get(&id).copied().unwrap_or(0),
                })
                .collect();
            // Oldest first: that's usually the original.
            files.sort_by_key(|f| f.mtime);
            groups.push(DupeGroup {
                size,
                wasted: size * (files.len() as u64 - 1),
                files,
            });
        }
    }
    groups.sort_by_key(|a| std::cmp::Reverse(a.wasted));
    groups.truncate(300);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_true_duplicates_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut data = vec![0u8; 300_000];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        let c = tmp.path().join("c.bin");
        std::fs::write(&a, &data).unwrap();
        std::fs::write(&b, &data).unwrap();
        // Same size, same head and tail, different middle.
        data[150_000] ^= 0xFF;
        std::fs::write(&c, &data).unwrap();
        let files = vec![(1, a, 300_000), (2, b, 300_000), (3, c, 300_000)];
        let progress = DupeProgress {
            total_bytes: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
        };
        let groups = find(files, &HashMap::new(), &progress, &AtomicBool::new(false));
        assert_eq!(groups.len(), 1);
        let ids: Vec<u32> = groups[0].files.iter().map(|f| f.id).collect();
        assert!(ids.contains(&1) && ids.contains(&2) && !ids.contains(&3));
        assert_eq!(groups[0].wasted, 300_000);
    }
}
