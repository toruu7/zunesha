//! Codebase map: a compact tree of the repository handed to every agent
//! (Phase 10).
//!
//! Two consumers, one source of truth:
//!
//! * **The workspace** — [`install`] writes `CODEBASE_MAP.md` into every
//!   workspace and excludes it from that workspace's git, so it never enters
//!   a baseline commit, a diff, or an approval. It is scaffolding, not the
//!   user's code.
//! * **The prompt** — [`read`] hands the same file to the worker, which
//!   appends it to every agent prompt (normal attempts and conflict
//!   resolutions alike), so the agent orients itself before touching a file.
//!
//! The walk uses the [`ignore`] crate so the map mirrors what the workspace
//! snapshot contains: `.gitignore` rules apply inside a git repository
//! (matching `git ls-files --exclude-standard`) and are inert in plain
//! directories (matching the snapshot fallback), hidden files are included,
//! `.git` / `.zou` are skipped, and empty directories are pruned
//! (the snapshot never copies them).
//!
//! Output is deliberately bounded — a map is reference material and must
//! never crowd the task out of a prompt or slow workspace creation:
//! `MAX_BYTES` caps the file, `MAX_ENTRIES` caps the walk, and line counting
//! reads at most `MAX_COUNT_BYTES` per call.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::error::Result;

/// File name of the map inside a workspace.
pub const MAP_FILE: &str = "CODEBASE_MAP.md";

/// Hard cap on the generated file (bytes). Mirrors the 8 KB discipline used
/// for validation-failure tails: reference material stays bounded.
const MAX_BYTES: usize = 8 * 1024;

/// Budget for the tree body, leaving room for header, fence, and stats.
const MAX_TREE_BYTES: usize = MAX_BYTES - 512;

/// Walk cap: a pathological repository must not slow workspace creation.
const MAX_ENTRIES: usize = 10_000;

/// Files bigger than this are shown by size; they are never read.
const MAX_LINES_FILE: u64 = 512 * 1024;

/// Total bytes [`generate`] will read for line counting (per call).
const MAX_COUNT_BYTES: u64 = 16 * 1024 * 1024;

/// One entry of the tree walk.
struct Node {
    /// Path relative to the root, `/`-separated (also the key a directory
    /// is looked up by when rendering its children).
    rel: String,
    /// Final path component, for display and sorting.
    name: String,
    kind: Kind,
    /// Line count for readable text files, `None` when skipped (binary,
    /// too large, read budget exhausted, symlink).
    lines: Option<u64>,
    /// File size in bytes (files only; kept for the size fallback label).
    size: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Symlink,
}

/// Aggregated figures for a directory subtree, filled deepest-first so a
/// parent can sum its children in one pass.
#[derive(Default, Clone, Copy)]
struct DirStats {
    files: usize,
    lines: u64,
}

/// Generate the map for `root` and return it as markdown.
///
/// Best-effort by design: entries that cannot be read or statted are
/// skipped, so a single unreadable file never fails workspace creation.
pub fn generate(root: &Path) -> String {
    let nodes = collect(root);
    render(nodes)
}

/// Write the map into `workspace` and keep it out of that workspace's git.
///
/// The exclude entry goes into the workspace's own `.git/info/exclude`, so
/// the user's ignore rules are untouched and `git add --all` (baseline,
/// diffs, conflict prep) silently skips the file. Requires an initialized
/// workspace (`.git/info` exists — `git init` creates it). Idempotent: the
/// file is regenerated and the exclude entry appears exactly once.
pub fn install(workspace: &Path, root: &Path) -> Result<()> {
    std::fs::write(workspace.join(MAP_FILE), generate(root))?;

    let exclude = workspace.join(".git").join("info").join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !existing.lines().any(|line| line.trim() == MAP_FILE) {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)?;
        writeln!(file, "{MAP_FILE}")?;
    }
    Ok(())
}

/// The installed map for prompt injection, if the workspace has one.
///
/// Workspaces from before Phase 10 simply have no map; callers degrade to a
/// map-less prompt. Trailing whitespace is trimmed so callers can append it
/// directly after `"\n\n"`.
pub fn read(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace.join(MAP_FILE)).ok()?;
    let text = text.trim_end();
    (!text.is_empty()).then(|| text.to_string())
}

/// Walk `root`, collecting display nodes.
fn collect(root: &Path) -> Vec<Node> {
    let mut budget = MAX_COUNT_BYTES;
    let mut nodes = Vec::new();

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        // Hidden files are part of the snapshot (`.gitignore`, `.env`, …).
        .hidden(false)
        // The root is the boundary; do not inherit ignore rules from above.
        .parents(false)
        // Control directories and the map itself are invisible scaffolding.
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git" && name != ".zou" && name != MAP_FILE
        });

    for entry in builder.build() {
        if nodes.len() >= MAX_ENTRIES {
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let kind = if file_type.is_dir() {
            Kind::Dir
        } else if file_type.is_symlink() {
            Kind::Symlink
        } else {
            Kind::File
        };
        let (lines, size) = if kind == Kind::File {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            (count_lines(entry.path(), size, &mut budget), size)
        } else {
            (None, 0)
        };
        nodes.push(Node {
            rel,
            name,
            kind,
            lines,
            size,
        });
    }
    nodes
}

/// Line count for a text file, or `None` when counting is skipped: the file
/// is binary (NUL in the first 8 KB — the classic heuristic), too large, or
/// the per-call read budget is spent. Budget is charged for every read that
/// happens, successful or not.
fn count_lines(path: &Path, size: u64, budget: &mut u64) -> Option<u64> {
    if size == 0 {
        return Some(0);
    }
    if size > MAX_LINES_FILE || size > *budget {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    *budget = budget.saturating_sub(bytes.len() as u64);
    if bytes.iter().take(8 * 1024).any(|&b| b == 0) {
        return None;
    }
    let mut lines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    if bytes.last() != Some(&b'\n') {
        lines += 1;
    }
    Some(lines)
}

/// Assemble the final markdown document from walked nodes.
fn render(nodes: Vec<Node>) -> String {
    // Group entries under their parent directory, dirs first then by name.
    let mut by_dir: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    for node in nodes {
        let parent = match node.rel.rfind('/') {
            Some(i) => node.rel[..i].to_string(),
            None => String::new(),
        };
        by_dir.entry(parent).or_default().push(node);
    }
    for kids in by_dir.values_mut() {
        kids.sort_by(|a, b| {
            let a_dir = matches!(a.kind, Kind::Dir);
            let b_dir = matches!(b.kind, Kind::Dir);
            b_dir.cmp(&a_dir).then_with(|| a.name.cmp(&b.name))
        });
    }

    // Deepest directories first, so a parent sees finished child stats. A
    // directory with no files beneath it gets no stats entry, which drops it
    // from the tree — the snapshot never copies empty directories.
    let mut keys: Vec<String> = by_dir.keys().cloned().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(depth(k)));
    let mut stats: HashMap<String, DirStats> = HashMap::new();
    for key in &keys {
        let mut agg = DirStats::default();
        let mut any = false;
        for node in by_dir.get(key).expect("key came from this map") {
            match node.kind {
                Kind::Dir => {
                    if let Some(sub) = stats.get(&node.rel) {
                        agg.files += sub.files;
                        agg.lines += sub.lines;
                        any = true;
                    }
                }
                Kind::File | Kind::Symlink => {
                    agg.files += 1;
                    agg.lines += node.lines.unwrap_or(0);
                    any = true;
                }
            }
        }
        // The root is always kept, even for an (almost) empty repository.
        if any || key.is_empty() {
            stats.insert(key.clone(), agg);
        }
    }
    if !stats.contains_key("") {
        stats.insert(String::new(), DirStats::default());
    }

    // Entries the tree will show, for the truncation note.
    let total: usize = stats
        .keys()
        .map(|key| {
            by_dir
                .get(key)
                .map(|kids| {
                    kids.iter()
                        .filter(|n| !matches!(n.kind, Kind::Dir) || stats.contains_key(&n.rel))
                        .count()
                })
                .unwrap_or(0)
        })
        .sum();

    let root_stats = stats.get("").copied().unwrap_or_default();
    let dir_count = stats.len().saturating_sub(1);

    let mut out = String::with_capacity(MAX_BYTES);
    out.push_str("# Codebase map\n\n");
    out.push_str(
        "Repository tree of this workspace's snapshot; gitignore rules apply \
         when the source is a git repository. Generated scaffolding, \
         git-excluded: it never enters a commit or a diff.\n\n",
    );
    out.push_str("```text\n.\n");

    let mut painter = Painter {
        out: String::new(),
        shown: 0,
        budget: MAX_TREE_BYTES,
        truncated: false,
    };
    paint("", "", &by_dir, &stats, &mut painter);
    out.push_str(&painter.out);
    if painter.truncated {
        out.push_str(&format!(
            "… (truncated — {} entries not shown)\n",
            total.saturating_sub(painter.shown)
        ));
    }
    out.push_str("```\n\n");

    let unit = if dir_count == 1 {
        "directory"
    } else {
        "directories"
    };
    out.push_str(&format!(
        "{dir_count} {unit} · {} file{} · {} line{}\n",
        root_stats.files,
        plural(root_stats.files as u64),
        root_stats.lines,
        plural(root_stats.lines)
    ));
    out
}

/// Rendered tree body with an 8 KB budget and truncation bookkeeping.
struct Painter {
    out: String,
    /// Child entries written so far (the root line is not counted).
    shown: usize,
    budget: usize,
    truncated: bool,
}

impl Painter {
    /// Append one tree line; returns `false` once the budget is spent.
    fn push_line(&mut self, line: &str) -> bool {
        if self.out.len() + line.len() + 1 > self.budget {
            self.truncated = true;
            return false;
        }
        self.out.push_str(line);
        self.out.push('\n');
        self.shown += 1;
        true
    }
}

/// Depth of a directory key: `""` is 0, `"src"` is 1, `"src/lib"` is 2.
fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.matches('/').count() + 1
    }
}

/// Recursively paint `key`'s children with `tree`-style guides.
///
/// Directories without stats (empty or pruned) are filtered out here, so the
/// render always agrees with the stats pass.
fn paint(
    key: &str,
    prefix: &str,
    by_dir: &BTreeMap<String, Vec<Node>>,
    stats: &HashMap<String, DirStats>,
    painter: &mut Painter,
) {
    let Some(kids) = by_dir.get(key) else { return };
    let shown_kids: Vec<&Node> = kids
        .iter()
        .filter(|n| !matches!(n.kind, Kind::Dir) || stats.contains_key(&n.rel))
        .collect();
    for (i, node) in shown_kids.iter().enumerate() {
        let last = i + 1 == shown_kids.len();
        let (branch, child_prefix) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        let label = match node.kind {
            Kind::Dir => {
                let s = stats[&node.rel];
                if s.lines > 0 {
                    format!(
                        "{prefix}{branch}{}/ ({} file{} · {} line{})",
                        node.name,
                        s.files,
                        plural(s.files as u64),
                        s.lines,
                        plural(s.lines)
                    )
                } else {
                    format!(
                        "{prefix}{branch}{}/ ({} file{})",
                        node.name,
                        s.files,
                        plural(s.files as u64)
                    )
                }
            }
            Kind::Symlink => format!("{prefix}{branch}{} (symlink)", node.name),
            Kind::File => {
                let detail = match node.lines {
                    Some(n) => format!("{n} line{}", plural(n)),
                    None => fmt_size(node.size),
                };
                format!("{prefix}{branch}{} ({detail})", node.name)
            }
        };
        if !painter.push_line(&label) {
            return;
        }
        if matches!(node.kind, Kind::Dir) {
            paint(
                &node.rel,
                &format!("{prefix}{child_prefix}"),
                by_dir,
                stats,
                painter,
            );
            if painter.truncated {
                return;
            }
        }
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Human-readable byte size for files without a line count.
fn fmt_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", b / KB)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A git repository whose `.gitignore` hides `build/`, with a control
    /// directory and an empty directory that must not appear.
    fn scratch_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} should succeed");
        };
        run(&["init", "--quiet"]);
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(root.join("README.md"), "# demo\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(root.join("src").join("mod.rs"), "mod a;\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build").join("artifact.bin"), "junk").unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::create_dir_all(root.join(".zou")).unwrap();
        std::fs::write(root.join(".zou").join("state.db"), "…").unwrap();
        tmp
    }

    #[test]
    fn map_mirrors_the_snapshot_rules() {
        let tmp = scratch_repo();
        let map = generate(tmp.path());

        // Present: files with exact line counts, aggregated directories.
        assert!(map.contains("README.md (1 line)"), "{map}");
        assert!(map.contains(".env (1 line)"), "{map}");
        assert!(map.contains("│   ├── lib.rs (2 lines)"), "{map}");
        assert!(map.contains("src/ (2 files · 3 lines)"), "{map}");
        assert!(map.contains("# Codebase map"), "{map}");

        // Absent: gitignored content, control dirs, empty directories.
        assert!(!map.contains("artifact.bin"), "{map}");
        assert!(!map.contains(".zou"), "{map}");
        assert!(!map.contains(".git/"), "{map}");
        assert!(!map.contains("empty"), "{map}");

        // Totals: README + .env + .gitignore + 2 files under src.
        assert!(map.contains("1 directory · 5 files · 6 lines"), "{map}");
    }

    #[test]
    fn map_treats_gitignore_as_inert_without_a_repository() {
        // `zou init --force` (plain directory) has no git semantics to
        // consult — the snapshot fallback copies everything, so does the map.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(root.join("README.md"), "# demo\n").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build").join("artifact.bin"), "junk").unwrap();

        let map = generate(root);
        assert!(map.contains("artifact.bin (1 line)"), "{map}");
        assert!(map.contains("README.md (1 line)"), "{map}");
    }

    #[test]
    fn map_shows_sizes_for_binaries_and_empty_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("blob.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(root.join("empty.txt"), b"").unwrap();
        std::fs::write(root.join("no-newline.txt"), "tail").unwrap();

        let map = generate(root);
        assert!(map.contains("blob.bin (3 B)"), "{map}");
        assert!(map.contains("empty.txt (0 lines)"), "{map}");
        assert!(map.contains("no-newline.txt (1 line)"), "{map}");
    }

    #[cfg(unix)]
    #[test]
    fn map_labels_symlinks_without_following_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("real.rs"), "fn x() {}\n").unwrap();
        std::os::unix::fs::symlink("real.rs", root.join("alias.rs")).unwrap();

        let map = generate(root);
        assert!(map.contains("alias.rs (symlink)"), "{map}");
    }

    #[test]
    fn map_stays_within_its_byte_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..600 {
            std::fs::write(root.join(format!("file_{i:04}_longname.rs")), "// filler\n").unwrap();
        }

        let map = generate(root);
        assert!(map.len() <= MAX_BYTES, "map is {} bytes", map.len());
        assert!(map.contains("truncated"), "{map}");
        // The stats line still reports what was collected.
        assert!(map.contains("600 files"), "{map}");
    }

    #[test]
    fn install_writes_a_readable_map() {
        let tmp = scratch_repo();
        let ws = tempfile::tempdir().unwrap();
        // `install` needs the `.git/info` directory a workspace has.
        std::fs::create_dir_all(ws.path().join(".git").join("info")).unwrap();

        install(ws.path(), tmp.path()).unwrap();
        let text = read(ws.path()).expect("map should be readable");
        assert!(text.contains("# Codebase map"));
        assert!(!text.ends_with('\n'), "read() trims trailing whitespace");
    }

    #[test]
    fn read_returns_none_for_missing_or_empty_maps() {
        let ws = tempfile::tempdir().unwrap();
        assert!(read(ws.path()).is_none());
        std::fs::write(ws.path().join(MAP_FILE), "   \n").unwrap();
        assert!(read(ws.path()).is_none());
    }
}
