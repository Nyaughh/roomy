//! Read-only views over a scanned tree, shaped for the UI.

use crate::tree::*;
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,
    pub size: u64,
    pub logical: u64,
    pub files: u32,
    pub kind: &'static str,
    pub mtime: i64,
    pub has_children: bool,
    pub denied: bool,
    pub hardlink: bool,
    pub mount: bool,
}

pub fn kind_name(k: u8) -> &'static str {
    match k {
        KIND_FILE => "file",
        KIND_DIR => "dir",
        KIND_LINK => "link",
        KIND_GAP => "gap",
        _ => "gapPart",
    }
}

pub fn info(tree: &Tree, id: u32) -> NodeInfo {
    let n = &tree.nodes[id as usize];
    NodeInfo {
        id,
        name: tree.name(id).to_string(),
        size: n.size,
        logical: n.logical,
        files: n.files,
        kind: kind_name(n.kind),
        mtime: n.mtime,
        has_children: tree.children(id).next().is_some(),
        denied: n.flags & FLAG_DENIED != 0,
        hardlink: n.flags & FLAG_HARDLINK != 0,
        mount: n.flags & FLAG_MOUNT != 0,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rest {
    pub count: u32,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Children {
    pub parent: NodeInfo,
    pub items: Vec<NodeInfo>,
    pub rest: Option<Rest>,
}

pub fn children(tree: &Tree, id: u32, limit: usize) -> Children {
    let kids = tree.sorted_children(id);
    let rest = (kids.len() > limit).then(|| Rest {
        count: (kids.len() - limit) as u32,
        size: kids[limit..]
            .iter()
            .map(|&c| tree.nodes[c as usize].size)
            .sum(),
    });
    Children {
        parent: info(tree, id),
        items: kids.iter().take(limit).map(|&c| info(tree, c)).collect(),
        rest,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapNode {
    /// None for an aggregate of many small items.
    pub id: Option<u32>,
    pub name: String,
    pub size: u64,
    pub kind: &'static str,
    pub count: u32,
    pub children: Vec<TreemapNode>,
}

/// Nested layout data for the treemap: `depth` levels below `id`, with items
/// smaller than `min_frac` of the root folded into one "smaller items" tile.
pub fn treemap(tree: &Tree, id: u32, depth: u32, min_frac: f64) -> TreemapNode {
    let total = tree.nodes[id as usize].size.max(1);
    let min = ((total as f64) * min_frac) as u64;
    build_tm(tree, id, depth, min.max(1))
}

fn build_tm(tree: &Tree, id: u32, depth: u32, min: u64) -> TreemapNode {
    let n = &tree.nodes[id as usize];
    let mut node = TreemapNode {
        id: Some(id),
        name: tree.name(id).to_string(),
        size: n.size,
        kind: kind_name(n.kind),
        count: 1,
        children: Vec::new(),
    };
    if depth == 0 {
        return node;
    }
    let kids = tree.sorted_children(id);
    let mut rest_size = 0u64;
    let mut rest_count = 0u32;
    for (i, &c) in kids.iter().enumerate() {
        let cs = tree.nodes[c as usize].size;
        if cs == 0 {
            continue;
        }
        if cs >= min && i < 300 {
            // Only descend into children big enough to show detail.
            let d = if cs >= min * 16 { depth - 1 } else { 0 };
            node.children.push(build_tm(tree, c, d, min));
        } else {
            rest_size += cs;
            rest_count += 1;
        }
    }
    if rest_size > 0 {
        node.children.push(TreemapNode {
            id: None,
            name: format!("{rest_count} smaller items"),
            size: rest_size,
            kind: "rest",
            count: rest_count,
            children: Vec::new(),
        });
    }
    node
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LargeFile {
    pub node: NodeInfo,
    pub folder: String,
}

pub fn largest_files(tree: &Tree, limit: usize) -> Vec<LargeFile> {
    let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::with_capacity(limit + 1);
    for (i, n) in tree.nodes.iter().enumerate() {
        if n.kind != KIND_FILE || n.is_deleted() {
            continue;
        }
        if heap.len() < limit {
            heap.push(Reverse((n.size, i as u32)));
        } else if let Some(Reverse((min, _))) = heap.peek() {
            if n.size > *min {
                heap.pop();
                heap.push(Reverse((n.size, i as u32)));
            }
        }
    }
    let mut v: Vec<(u64, u32)> = heap.into_iter().map(|Reverse(x)| x).collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.0));
    v.into_iter()
        .map(|(_, id)| {
            let parent = tree.nodes[id as usize].parent;
            LargeFile {
                node: info(tree, id),
                folder: tree.path(parent).to_string_lossy().into_owned(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtStat {
    pub ext: String,
    pub size: u64,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeStat {
    pub category: &'static str,
    pub size: u64,
    pub count: u64,
    pub exts: Vec<ExtStat>,
}

pub fn category_of(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "pagefile.sys" | "hiberfil.sys" | "swapfile.sys" | "memory.dmp"
    ) {
        return "System";
    }
    let ext = match lower.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e,
        _ => return "Other",
    };
    match ext {
        "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" | "m4v" | "flv" | "mpg" | "mpeg" | "ts"
        | "m2ts" | "3gp" => "Video",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "bmp" | "tif" | "tiff"
        | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" | "psd" | "svg" | "ico"
        | "avif" => "Images",
        "mp3" | "flac" | "wav" | "aac" | "m4a" | "ogg" | "opus" | "wma" | "aiff" | "aif"
        | "alac" | "mid" => "Audio",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "rtf" | "odt"
        | "ods" | "odp" | "csv" | "epub" | "pages" | "numbers" | "key" | "one" | "pst" | "ost" => {
            "Documents"
        }
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "iso" | "dmg"
        | "cab" | "lz4" => "Archives",
        "exe" | "msi" | "dll" | "app" | "pkg" | "appx" | "msix" | "msixbundle" | "apk" | "jar"
        | "deb" | "rpm" | "so" | "dylib" | "node" | "pdb" | "lib" | "a" | "o" | "obj" | "rlib"
        | "rmeta" => "Apps & code binaries",
        "js" | "mjs" | "cjs" | "ts_" | "tsx" | "jsx" | "py" | "rs" | "go" | "java" | "kt" | "c"
        | "cc" | "cpp" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "json" | "yaml" | "yml"
        | "toml" | "xml" | "html" | "css" | "scss" | "map" | "lock" | "sql" | "sh" | "ps1"
        | "wasm" => "Code",
        "vhd" | "vhdx" | "vmdk" | "vdi" | "qcow2" | "img" | "avhdx" => "Virtual disks",
        "pak" | "vpk" | "ucas" | "utoc" | "uasset" | "umap" | "bundle" | "assets" | "unity3d"
        | "bsa" | "ba2" | "esm" | "forge" | "big" | "wad" | "rpf" | "arc" => "Game data",
        "db" | "sqlite" | "sqlite3" | "mdb" | "accdb" | "ldf" | "mdf" | "ldb" | "edb" | "dat"
        | "log" | "etl" | "evtx" | "tmp" | "cache" | "blob" => "Data & logs",
        "sys" | "cat" | "mui" | "inf" | "manifest" | "drv" | "cpl" | "msp" | "efi" => "System",
        _ => "Other",
    }
}

pub fn file_types(tree: &Tree) -> Vec<TypeStat> {
    type Totals = (u64, u64, HashMap<String, (u64, u64)>);
    let mut cats: HashMap<&'static str, Totals> = HashMap::new();
    for (i, n) in tree.nodes.iter().enumerate() {
        if n.is_deleted() {
            continue;
        }
        let (cat, ext) = match n.kind {
            KIND_FILE => {
                let name = tree.name(i as u32);
                let ext = name
                    .rsplit_once('.')
                    .filter(|(s, e)| !s.is_empty() && e.len() <= 12)
                    .map(|(_, e)| format!(".{}", e.to_ascii_lowercase()))
                    .unwrap_or_else(|| "(no extension)".into());
                (category_of(name), ext)
            }
            KIND_GAP_PART => ("Hidden system space", tree.name(i as u32).to_string()),
            _ => continue,
        };
        let e = cats.entry(cat).or_default();
        e.0 += n.size;
        e.1 += u64::from(n.kind == KIND_FILE);
        let x = e.2.entry(ext).or_default();
        x.0 += n.size;
        x.1 += 1;
    }
    let mut out: Vec<TypeStat> = cats
        .into_iter()
        .map(|(category, (size, count, exts))| {
            let mut exts: Vec<ExtStat> = exts
                .into_iter()
                .map(|(ext, (size, count))| ExtStat { ext, size, count })
                .collect();
            exts.sort_by_key(|a| std::cmp::Reverse(a.size));
            exts.truncate(12);
            TypeStat {
                category,
                size,
                count,
                exts,
            }
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.size));
    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub id: u32,
    pub name: String,
}

pub fn crumbs(tree: &Tree, id: u32) -> Vec<Crumb> {
    tree.ancestry(id)
        .into_iter()
        .map(|c| Crumb {
            id: c,
            name: tree.name(c).to_string(),
        })
        .collect()
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// Find the live node for an absolute path inside the scanned tree.
pub fn find_by_path(tree: &Tree, path: &Path) -> Option<u32> {
    // Registry values sometimes use forward slashes ("C:/Riot Games/...").
    let fixed;
    let path = if cfg!(windows) && path.to_string_lossy().contains('/') {
        fixed = PathBuf::from(path.to_string_lossy().replace('/', "\\"));
        fixed.as_path()
    } else {
        path
    };
    let root = Path::new(&tree.root_path);
    let rel = if cfg!(windows) {
        let p = path.to_string_lossy().to_lowercase();
        let r = tree.root_path.to_lowercase();
        let r = r.trim_end_matches('\\');
        let rest = p.strip_prefix(r)?;
        if !(rest.is_empty() || rest.starts_with('\\')) {
            return None;
        }
        // Map back to the original-case suffix.
        let orig = path.to_string_lossy();
        orig[orig.len() - rest.len()..]
            .trim_start_matches('\\')
            .to_string()
    } else {
        path.strip_prefix(root).ok()?.to_string_lossy().into_owned()
    };
    let mut cur = 0u32;
    for comp in Path::new(&rel).components() {
        if let Component::Normal(name) = comp {
            let name = name.to_string_lossy();
            cur = tree
                .children(cur)
                .find(|&c| same_name(tree.name(c), &name))?;
        }
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, size: u64) -> RawEntry {
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
        let mut sub = RawDir::new("Sub".into(), 0);
        sub.files = (0..50).map(|i| f(&format!("f{i}.mp4"), 1000 + i)).collect();
        sub.finish();
        let mut root = RawDir::new("r".into(), 0);
        root.files.push(f("doc.pdf", 10));
        root.files.push(f("noext", 3));
        root.dirs.push(sub);
        root.finish();
        let root_path = if cfg!(windows) { r"C:\data" } else { "/data" };
        Tree::from_raw(root, root_path.into())
    }

    #[test]
    fn largest_and_types() {
        let t = sample();
        let l = largest_files(&t, 3);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].node.name, "f49.mp4");
        assert!(l[0].node.size >= l[1].node.size);
        let types = file_types(&t);
        assert_eq!(types[0].category, "Video");
        assert_eq!(types[0].count, 50);
        assert!(types.iter().any(|c| c.category == "Documents"));
    }

    #[test]
    fn treemap_aggregates_small_items() {
        let t = sample();
        let tm = treemap(&t, 0, 2, 0.05);
        let sub = &tm.children[0];
        assert_eq!(sub.name, "Sub");
        // Each file is ~2% of the total, so they fold into one tile.
        assert!(sub.children.iter().any(|c| c.kind == "rest"));
        let total: u64 = tm.children.iter().map(|c| c.size).sum();
        assert_eq!(total, t.nodes[0].size, "nothing is lost when aggregating");
    }

    #[test]
    fn path_lookup() {
        let t = sample();
        let p = t.path(t.sorted_children(t.child_by_name(0, "Sub").unwrap())[0]);
        let id = find_by_path(&t, &p).unwrap();
        assert_eq!(t.name(id), "f49.mp4");
        if cfg!(windows) {
            assert!(find_by_path(&t, Path::new(r"c:\DATA\sub")).is_some());
            assert!(
                find_by_path(&t, Path::new("C:/data/Sub")).is_some(),
                "registry paths may use forward slashes"
            );
        }
        assert_eq!(find_by_path(&t, &t.path(0)), Some(0));
    }
}
