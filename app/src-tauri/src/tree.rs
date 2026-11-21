//! Compact arena representation of a scanned directory tree.
//!
//! Every node lives in one `Vec<Node>`; names live in one shared string
//! buffer. Children of a directory occupy a contiguous id range, sorted by
//! size (largest first) when the tree is built.

use std::path::PathBuf;

pub const NONE: u32 = u32::MAX;

pub const KIND_FILE: u8 = 0;
pub const KIND_DIR: u8 = 1;
/// Junction, symlink or mount point. Never followed, never counted.
pub const KIND_LINK: u8 = 2;
/// Pseudo node: used space that the scan could not attribute to a file.
pub const KIND_GAP: u8 = 3;
/// Pseudo node: a known part of the gap (e.g. the NTFS file table).
pub const KIND_GAP_PART: u8 = 4;

pub const FLAG_DELETED: u8 = 1;
/// The directory could not be read (permission denied).
pub const FLAG_DENIED: u8 = 2;
/// A hard link to a file that was already counted elsewhere.
pub const FLAG_HARDLINK: u8 = 4;
/// Another volume is mounted here.
pub const FLAG_MOUNT: u8 = 8;

#[derive(Clone, Copy, Debug)]
pub struct Node {
    /// Bytes allocated on disk (hard links counted once).
    pub size: u64,
    /// Logical (apparent) size in bytes.
    pub logical: u64,
    /// Last modification time, seconds since the Unix epoch.
    pub mtime: i64,
    pub name_off: u32,
    pub parent: u32,
    pub first_child: u32,
    pub child_count: u32,
    /// Number of files in this subtree (1 for a file).
    pub files: u32,
    pub name_len: u16,
    pub kind: u8,
    pub flags: u8,
}

impl Node {
    pub fn is_deleted(&self) -> bool {
        self.flags & FLAG_DELETED != 0
    }
}

/// A file-like entry produced by the scanner.
#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: String,
    pub size: u64,
    pub logical: u64,
    pub mtime: i64,
    pub kind: u8,
    pub flags: u8,
}

/// A directory produced by the scanner, with subtree totals precomputed.
#[derive(Debug, Default)]
pub struct RawDir {
    pub name: String,
    pub mtime: i64,
    pub kind: u8,
    pub flags: u8,
    pub files: Vec<RawEntry>,
    pub dirs: Vec<RawDir>,
    pub total_size: u64,
    pub total_logical: u64,
    pub total_files: u64,
}

impl RawDir {
    pub fn new(name: String, mtime: i64) -> Self {
        RawDir {
            name,
            mtime,
            kind: KIND_DIR,
            ..Default::default()
        }
    }

    /// Recompute this directory's totals from its direct children (whose own
    /// totals must already be final).
    pub fn finish(&mut self) {
        let mut size = 0u64;
        let mut logical = 0u64;
        let mut files = 0u64;
        for f in &self.files {
            size += f.size;
            logical += f.logical;
            if f.kind == KIND_FILE {
                files += 1;
            }
        }
        for d in &self.dirs {
            size += d.total_size;
            logical += d.total_logical;
            files += d.total_files;
        }
        self.total_size = size;
        self.total_logical = logical;
        self.total_files = files;
    }
}

pub struct Tree {
    pub nodes: Vec<Node>,
    names: String,
    /// Absolute path of the root; also the root node's name.
    pub root_path: String,
    /// Bumped on every mutation so the UI can invalidate cached views.
    pub generation: u64,
}

impl Tree {
    pub fn from_raw(raw: RawDir, root_path: String) -> Tree {
        let mut t = Tree {
            nodes: Vec::with_capacity((raw.total_files as usize).saturating_add(1024)),
            names: String::new(),
            root_path: root_path.clone(),
            generation: 1,
        };
        let id = t.push_node(&root_path, raw.kind, raw.flags, NONE, raw.mtime);
        t.nodes[id as usize].size = raw.total_size;
        t.nodes[id as usize].logical = raw.total_logical;
        t.nodes[id as usize].files = clamp_u32(raw.total_files);
        t.fill(id, raw);
        t
    }

    fn push_node(&mut self, name: &str, kind: u8, flags: u8, parent: u32, mtime: i64) -> u32 {
        let off = self.names.len() as u32;
        let name = truncate_name(name);
        self.names.push_str(name);
        self.nodes.push(Node {
            size: 0,
            logical: 0,
            mtime,
            name_off: off,
            parent,
            first_child: NONE,
            child_count: 0,
            files: 0,
            name_len: name.len() as u16,
            kind,
            flags,
        });
        (self.nodes.len() - 1) as u32
    }

    /// Append `raw`'s children as the children of `id` (consumes `raw`).
    fn fill(&mut self, id: u32, raw: RawDir) {
        enum Child {
            File(usize),
            Dir(usize),
        }
        let RawDir { files, dirs, .. } = raw;
        let mut order: Vec<(u64, Child)> = Vec::with_capacity(files.len() + dirs.len());
        for (i, f) in files.iter().enumerate() {
            order.push((f.size, Child::File(i)));
        }
        for (i, d) in dirs.iter().enumerate() {
            order.push((d.total_size, Child::Dir(i)));
        }
        order.sort_by_key(|a| std::cmp::Reverse(a.0));

        let first = self.nodes.len() as u32;
        let mut pending: Vec<(u32, usize)> = Vec::with_capacity(dirs.len());
        for (_, c) in &order {
            match *c {
                Child::File(i) => {
                    let f = &files[i];
                    let cid = self.push_node(&f.name, f.kind, f.flags, id, f.mtime);
                    let n = &mut self.nodes[cid as usize];
                    n.size = f.size;
                    n.logical = f.logical;
                    n.files = u32::from(f.kind == KIND_FILE);
                }
                Child::Dir(i) => {
                    let d = &dirs[i];
                    let cid = self.push_node(&d.name, d.kind, d.flags, id, d.mtime);
                    let n = &mut self.nodes[cid as usize];
                    n.size = d.total_size;
                    n.logical = d.total_logical;
                    n.files = clamp_u32(d.total_files);
                    pending.push((cid, i));
                }
            }
        }
        let node = &mut self.nodes[id as usize];
        node.first_child = if order.is_empty() { NONE } else { first };
        node.child_count = order.len() as u32;
        drop(files);

        let mut dirs: Vec<Option<RawDir>> = dirs.into_iter().map(Some).collect();
        for (cid, i) in pending {
            if let Some(d) = dirs[i].take() {
                self.fill(cid, d);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, id: u32) -> Option<&Node> {
        self.nodes.get(id as usize).filter(|n| !n.is_deleted())
    }

    pub fn name(&self, id: u32) -> &str {
        let n = &self.nodes[id as usize];
        &self.names[n.name_off as usize..n.name_off as usize + n.name_len as usize]
    }

    /// Live children of `id`, in arena order.
    pub fn children(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let n = &self.nodes[id as usize];
        let (start, count) = if n.first_child == NONE {
            (0, 0)
        } else {
            (n.first_child, n.child_count)
        };
        (start..start + count).filter(move |&c| !self.nodes[c as usize].is_deleted())
    }

    /// Live children sorted by size, largest first.
    pub fn sorted_children(&self, id: u32) -> Vec<u32> {
        let mut v: Vec<u32> = self.children(id).collect();
        v.sort_by(|a, b| {
            self.nodes[*b as usize]
                .size
                .cmp(&self.nodes[*a as usize].size)
        });
        v
    }

    /// Ids from the root down to `id` (inclusive).
    pub fn ancestry(&self, id: u32) -> Vec<u32> {
        let mut v = vec![id];
        let mut cur = self.nodes[id as usize].parent;
        while cur != NONE {
            v.push(cur);
            cur = self.nodes[cur as usize].parent;
        }
        v.reverse();
        v
    }

    /// Absolute filesystem path of a node. Pseudo nodes have no real path.
    pub fn path(&self, id: u32) -> PathBuf {
        let chain = self.ancestry(id);
        let mut p = PathBuf::from(&self.root_path);
        for &c in chain.iter().skip(1) {
            p.push(self.name(c));
        }
        p
    }

    pub fn is_pseudo(&self, id: u32) -> bool {
        let chain = self.ancestry(id);
        chain.iter().any(|&c| {
            let k = self.nodes[c as usize].kind;
            k == KIND_GAP || k == KIND_GAP_PART
        })
    }

    fn mark_deleted(&mut self, id: u32) {
        let mut stack = vec![id];
        while let Some(c) = stack.pop() {
            let n = &mut self.nodes[c as usize];
            if n.is_deleted() && c != id {
                continue;
            }
            n.flags |= FLAG_DELETED;
            if n.first_child != NONE {
                let (s, k) = (n.first_child, n.child_count);
                stack.extend(s..s + k);
            }
        }
    }

    fn adjust_ancestors(&mut self, id: u32, d_size: i128, d_logical: i128, d_files: i64) {
        let mut cur = self.nodes[id as usize].parent;
        while cur != NONE {
            let n = &mut self.nodes[cur as usize];
            n.size = (n.size as i128 + d_size).max(0) as u64;
            n.logical = (n.logical as i128 + d_logical).max(0) as u64;
            n.files = (n.files as i64 + d_files).clamp(0, u32::MAX as i64) as u32;
            cur = n.parent;
        }
    }

    /// Remove a node (and its subtree), updating every ancestor's totals.
    pub fn remove(&mut self, id: u32) {
        if id == 0 || self.nodes[id as usize].is_deleted() {
            return;
        }
        let n = self.nodes[id as usize];
        self.adjust_ancestors(
            id,
            -(n.size as i128),
            -(n.logical as i128),
            -(n.files as i64),
        );
        self.mark_deleted(id);
        self.generation += 1;
    }

    /// Replace the contents of directory `id` with a freshly scanned subtree.
    pub fn replace_subtree(&mut self, id: u32, raw: RawDir) {
        let old = self.nodes[id as usize];
        if old.first_child != NONE {
            for c in old.first_child..old.first_child + old.child_count {
                self.mark_deleted(c);
            }
        }
        let d_size = raw.total_size as i128 - old.size as i128;
        let d_logical = raw.total_logical as i128 - old.logical as i128;
        let d_files = raw.total_files as i64 - old.files as i64;
        {
            let n = &mut self.nodes[id as usize];
            n.size = raw.total_size;
            n.logical = raw.total_logical;
            n.files = clamp_u32(raw.total_files);
            n.first_child = NONE;
            n.child_count = 0;
            n.flags = raw.flags;
        }
        self.fill(id, raw);
        self.adjust_ancestors(id, d_size, d_logical, d_files);
        self.generation += 1;
    }

    /// Find a direct child of `id` by name.
    pub fn child_by_name(&self, id: u32, name: &str) -> Option<u32> {
        self.children(id).find(|&c| self.name(c) == name)
    }
}

fn clamp_u32(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

fn truncate_name(name: &str) -> &str {
    if name.len() <= u16::MAX as usize {
        return name;
    }
    let mut end = u16::MAX as usize;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> RawEntry {
        RawEntry {
            name: name.into(),
            size,
            logical: size,
            mtime: 0,
            kind: KIND_FILE,
            flags: 0,
        }
    }

    fn sample() -> Tree {
        let mut sub = RawDir::new("sub".into(), 0);
        sub.files.push(file("big.bin", 500));
        sub.files.push(file("small.txt", 5));
        sub.finish();
        let mut root = RawDir::new("root".into(), 0);
        root.files.push(file("a.txt", 10));
        root.dirs.push(sub);
        root.finish();
        Tree::from_raw(root, "/tmp/root".into())
    }

    #[test]
    fn totals_and_order() {
        let t = sample();
        assert_eq!(t.nodes[0].size, 515);
        assert_eq!(t.nodes[0].files, 3);
        let kids = t.sorted_children(0);
        assert_eq!(t.name(kids[0]), "sub");
        assert_eq!(t.name(kids[1]), "a.txt");
        let sub = kids[0];
        assert_eq!(t.name(t.sorted_children(sub)[0]), "big.bin");
        assert_eq!(
            t.path(t.sorted_children(sub)[0]),
            PathBuf::from("/tmp/root").join("sub").join("big.bin")
        );
    }

    #[test]
    fn remove_updates_ancestors() {
        let mut t = sample();
        let sub = t.child_by_name(0, "sub").unwrap();
        let big = t.child_by_name(sub, "big.bin").unwrap();
        t.remove(big);
        assert_eq!(t.nodes[sub as usize].size, 5);
        assert_eq!(t.nodes[0].size, 15);
        assert_eq!(t.nodes[0].files, 2);
        assert!(t.get(big).is_none());
        assert_eq!(t.children(sub).count(), 1);
    }

    #[test]
    fn replace_subtree_updates_totals() {
        let mut t = sample();
        let sub = t.child_by_name(0, "sub").unwrap();
        let mut fresh = RawDir::new("sub".into(), 0);
        fresh.files.push(file("left.txt", 7));
        fresh.finish();
        t.replace_subtree(sub, fresh);
        assert_eq!(t.nodes[0].size, 17);
        assert_eq!(t.children(sub).count(), 1);
        assert!(t.child_by_name(sub, "big.bin").is_none());
        assert!(t.child_by_name(sub, "left.txt").is_some());
    }
}
