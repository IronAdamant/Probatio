//! Mutant trees and their build cache.
//!
//! A run takes one slot per worker. A slot is a directory in Probatio's cache, keyed by the git
//! checkout (or the package, with no git):
//!
//! - `tree/`: a git worktree of the checkout (or a plain copy), made once per run, used by every
//!   mutant of that worker, and removed when the run ends, is cancelled, or on the next start
//!   after a crash. A mutant edits one file in it; after the suite, every file that changed is
//!   copied back from your tree and every file the suite added is removed, so the next mutant
//!   starts from your tree again.
//! - `target/`: `CARGO_TARGET_DIR` for every mutant suite in this slot. It is kept between runs,
//!   so a second run builds only what changed.
//! - `lock`: the pid that holds the slot. A slot whose pid is gone is stale and is swept.
//! - `dirty.txt`: files a mutant has edited while builds went to `target/`.
//!
//! Cargo decides what to rebuild from file mtimes. The tree keeps your files' mtimes, so an
//! unchanged crate is fresh in `target/` from the last run. A file in `dirty.txt` may have been
//! compiled mutated into `target/`; it is given the current time whenever a tree is made, so
//! cargo rebuilds it from the clean text and a mutated artifact is never reused.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// Most slots per checkout: workers of one run plus runs at the same time.
const MAX_SLOTS: usize = 16;
/// A `dirty.txt` longer than this clears `target/` instead, so its list stays short.
const MAX_DIRTY: usize = 500;
/// A checkout's cache not used for this long is removed by the next sweep.
const STALE_CACHE: Duration = Duration::from_secs(14 * 24 * 3600);
/// A 0.7 temp worktree or copy older than this, with no owner, is removed by the next sweep.
const STALE_LEGACY: Duration = Duration::from_secs(3600);

/// `PROBATIO_CACHE_DIR`, else the platform's cache folder, else the temp folder.
pub fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("PROBATIO_CACHE_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if cfg!(test) {
        return std::env::temp_dir().join("probatio-test-cache");
    }
    let env = |key: &str| std::env::var_os(key).filter(|value| !value.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|home| home.join("Library").join("Caches"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|home| home.join(".cache")))
    };
    match base {
        Some(base) => base.join("probatio"),
        None => std::env::temp_dir().join("probatio-cache"),
    }
}

/// 16 hex digits naming a checkout. The same folder always gets the same key.
pub fn key_for(root: &Path) -> String {
    let path = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut text = path.display().to_string().replace('\\', "/");
    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// One worker's tree and target directory. Dropping it removes the tree and frees the slot.
pub struct Slot {
    pub dir: PathBuf,
    pub tree: PathBuf,
    pub target: PathBuf,
    source: PathBuf,
    git: bool,
    manifest: HashMap<String, (u64, SystemTime)>,
    /// Held open for the slot's life: the lock.
    _lock: fs::File,
}

/// Take `count` free slots for `source` (the git top level, or the package with no git). A slot
/// another live run holds is skipped. Stale slots and leftovers are swept once the locks are held.
pub fn acquire(source: &Path, git: bool, count: usize) -> Result<Vec<Slot>, String> {
    let key_dir = cache_root().join(key_for(source));
    fs::create_dir_all(&key_dir).map_err(|error| format!("{} could not be created: {error}", key_dir.display()))?;
    let _ = fs::write(key_dir.join("source.txt"), source.display().to_string());
    let mut slots = Vec::new();
    for index in 0..MAX_SLOTS {
        if slots.len() >= count {
            break;
        }
        let dir = key_dir.join(index.to_string());
        let _ = fs::create_dir_all(&dir);
        let Some(lock) = take_lock(&dir) else { continue };
        let _ = fs::write(dir.join("used"), "");
        slots.push(Slot {
            tree: dir.join("tree"),
            target: dir.join("target"),
            dir,
            source: source.to_path_buf(),
            git,
            manifest: HashMap::new(),
            _lock: lock,
        });
    }
    if slots.len() < count.max(1) {
        return Err(format!("all {MAX_SLOTS} mutant slots for this checkout are in use by other runs; wait for one to end, or set PROBATIO_CACHE_DIR"));
    }
    // After the locks are taken, so a sweep never takes this run's slots for stale ones.
    sweep(source);
    Ok(slots)
}

/// An exclusive lock on `<slot>/lock`, held for as long as the returned file is open. The
/// operating system releases it when the process ends, however it ends, so a slot whose lock can
/// be taken is free, and a slot left by a killed run is free again at once.
fn take_lock(dir: &Path) -> Option<fs::File> {
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("lock")).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

impl Slot {
    /// Make the tree: a worktree of HEAD with no checkout, filled from the working tree with each
    /// file's mtime kept, or a plain copy with no git. `extra` are more files or folders (from the
    /// top of the tree) to copy even though git ignores them. Then every dirty file gets the
    /// current time.
    pub fn prepare(&mut self, extra: &[String]) -> Result<(), String> {
        if fs::symlink_metadata(&self.tree).is_ok() || self.git && registered(&self.source, &self.tree) {
            remove_tree(&self.source, &self.tree, self.git);
        }
        if fs::symlink_metadata(&self.tree).is_ok() {
            // A process the last suite left behind still holds the old tree. Build beside it; the
            // sweep removes it once nothing holds it. Only the first build here is a full one.
            let free = (1..8).map(|n| self.dir.join(format!("tree-{n}"))).find(|other| remove_tree(&self.source, other, self.git));
            match free {
                Some(other) => self.tree = other,
                None => return Err(format!("{} is held by another process; end it, or set PROBATIO_CACHE_DIR to another folder", self.tree.display())),
            }
        }
        let mut files = if self.git {
            git(&self.source, &["worktree", "add", "--detach", "--no-checkout", &self.tree.display().to_string(), "HEAD"])
                .map_err(|error| format!("git worktree add failed: {error}"))?;
            // Fill the index from HEAD without touching files, so git in the tree sees your edits.
            let _ = git(&self.tree, &["reset", "-q"]);
            git_files(&self.source)?
        } else {
            fs::create_dir_all(&self.tree).map_err(|error| error.to_string())?;
            walk_files(&self.source)
        };
        for rel in extra {
            let path = self.source.join(rel);
            if path.is_dir() {
                files.extend(walk_all(&path).into_iter().map(|inner| format!("{}/{inner}", rel.trim_end_matches('/'))));
            } else if path.is_file() {
                files.push(rel.clone());
            }
        }
        files.sort();
        files.dedup();
        let mut wiped = false;
        let dirty = self.dirty();
        if dirty.len() > MAX_DIRTY {
            let _ = remove_dir_robust(&self.target);
            let _ = fs::remove_file(self.dir.join("dirty.txt"));
            wiped = true;
        }
        for rel in &files {
            self.copy_in(rel, true)?;
        }
        if !wiped {
            let now = SystemTime::now();
            for rel in dirty {
                let path = self.tree.join(&rel);
                if path.is_file() {
                    set_mtime(&path, now);
                    self.record(&rel);
                }
            }
        }
        Ok(())
    }

    /// A fresh checkout of `commit`, with nothing copied over it, for a run pinned to a commit.
    /// Every file gets the current time, so nothing built from another tree is reused.
    pub fn checkout(&mut self, commit: &str) -> Result<(), String> {
        remove_tree(&self.source, &self.tree, self.git);
        self.manifest.clear();
        git(&self.source, &["worktree", "add", "--detach", &self.tree.display().to_string(), commit])
            .map(|_| ())
            .map_err(|error| format!("git worktree add failed: {error}"))
    }

    /// Note a file a mutant is about to edit, before the edit, so a crash still leaves the record.
    pub fn mark_dirty(&self, rel: &str) {
        let rel = rel.replace('\\', "/");
        let mut dirty = self.dirty();
        if dirty.contains(&rel) {
            return;
        }
        dirty.push(rel);
        let _ = fs::write(self.dir.join("dirty.txt"), format!("{}\n", dirty.join("\n")));
    }

    fn dirty(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("dirty.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// After a mutant: copy back every file whose size or mtime changed (the mutated file, and
    /// anything the suite wrote), with the current time, and remove what the suite added.
    pub fn refresh(&mut self) -> Result<(), String> {
        let changed: Vec<String> = self
            .manifest
            .iter()
            .filter(|(rel, recorded)| stat(&self.tree.join(rel)).as_ref() != Some(recorded))
            .map(|(rel, _)| rel.clone())
            .collect();
        for rel in changed {
            self.mark_dirty(&rel);
            self.copy_in(&rel, false)?;
        }
        let tree = self.tree.clone();
        self.remove_extras(&tree, "");
        Ok(())
    }

    fn copy_in(&mut self, rel: &str, keep_mtime: bool) -> Result<(), String> {
        let from = self.source.join(rel);
        let to = self.tree.join(rel);
        let Ok(meta) = fs::symlink_metadata(&from) else {
            // Deleted from your tree since the list was read: it is not in the tree either.
            let _ = fs::remove_file(&to);
            self.manifest.remove(rel);
            return Ok(());
        };
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{} could not be created: {error}", parent.display()))?;
        }
        if meta.file_type().is_symlink() {
            // A tracked symlink stays a link to the same target, as a checkout would make it.
            if fs::symlink_metadata(&to).is_err() {
                if let Ok(target) = fs::read_link(&from) {
                    link_like(&from, &target, &to);
                }
            }
            return Ok(());
        }
        if !meta.is_file() {
            return Ok(());
        }
        clear_readonly(&to);
        fs::copy(&from, &to).map_err(|error| format!("{rel} could not be copied into the mutant tree: {error}"))?;
        clear_readonly(&to);
        let when = if keep_mtime { meta.modified().unwrap_or_else(|_| SystemTime::now()) } else { SystemTime::now() };
        set_mtime(&to, when);
        self.record(rel);
        Ok(())
    }

    fn record(&mut self, rel: &str) {
        if let Some(seen) = stat(&self.tree.join(rel)) {
            self.manifest.insert(rel.to_string(), seen);
        }
    }

    fn remove_extras(&mut self, dir: &Path, prefix: &str) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if prefix.is_empty() && name == ".git" {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() || (kind.is_dir() && name == "node_modules") {
                continue;
            }
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if kind.is_dir() {
                self.remove_extras(&entry.path(), &rel);
                if !self.manifest.keys().any(|known| known.starts_with(&format!("{rel}/"))) {
                    let _ = remove_dir_robust(&entry.path());
                }
            } else if !self.manifest.contains_key(&rel) {
                clear_readonly(&entry.path());
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        remove_tree(&self.source, &self.tree, self.git);
        let _ = fs::write(self.dir.join("used"), "");
    }
}

/// Tracked files (as they are in the working tree) and untracked files git does not ignore,
/// relative to the top level. Ignored files (build output, caches, local state) are not source
/// and are not copied. An embedded checkout, such as another worktree inside this one, is not
/// part of this checkout and is skipped.
fn git_files(root: &Path) -> Result<Vec<String>, String> {
    let list = |args: &[&str]| git(root, args).map_err(|error| format!("git ls-files failed: {error}"));
    let tracked = list(&["ls-files", "-z", "--cached"])?;
    let untracked = list(&["ls-files", "-z", "--others", "--exclude-standard"])?;
    let keep = |rel: &&str| !rel.is_empty() && !rel.ends_with('/') && !rel.split('/').any(|part| part == ".probatio" || part == ".git");
    // An untracked `node_modules`, `target`, or `dist` that git was not told to ignore is still
    // installed packages or build output: `node_modules` is linked in, and the rest is not source.
    let mut files: Vec<String> = tracked.split('\0').filter(keep).map(str::to_string).collect();
    files.extend(untracked.split('\0').filter(keep).filter(|rel| !rel.split('/').any(crate::runner::skipped_in_copy)).map(str::to_string));
    files.sort();
    files.dedup();
    Ok(files)
}

/// With no git, every file under the package except the directories a copy always skips.
fn walk_files(root: &Path) -> Vec<String> {
    walk_with(root, true)
}

/// Every file under a folder named in `copy`, with nothing skipped but `.git`.
fn walk_all(root: &Path) -> Vec<String> {
    walk_with(root, false)
}

fn walk_with(root: &Path, skip: bool) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, skip: bool, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" || (skip && crate::runner::skipped_in_copy(&name)) {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            if kind.is_dir() {
                walk(&entry.path(), &rel, skip, out);
            } else if kind.is_file() {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", skip, &mut out);
    out.sort();
    out
}

/// A symlink at `to` with the same target as `from`. Windows needs Developer Mode or admin for a
/// symlink; without it, a link to a file is copied as that file.
fn link_like(from: &Path, target: &Path, to: &Path) {
    #[cfg(unix)]
    {
        let _ = (from, std::os::unix::fs::symlink(target, to));
    }
    #[cfg(windows)]
    {
        let made = if from.is_dir() { std::os::windows::fs::symlink_dir(target, to) } else { std::os::windows::fs::symlink_file(target, to) };
        if made.is_err() && from.is_file() {
            let _ = fs::copy(from, to);
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (from, target, to);
    }
}

fn stat(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = fs::symlink_metadata(path).ok()?;
    meta.is_file().then(|| (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

fn set_mtime(path: &Path, when: SystemTime) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(when);
    }
}

fn clear_readonly(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        let mut permissions = meta.permissions();
        if permissions.readonly() && !meta.file_type().is_symlink() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
    }
}

/// True when git lists `tree` as a worktree of `repo`.
fn registered(repo: &Path, tree: &Path) -> bool {
    let Ok(list) = git(repo, &["worktree", "list", "--porcelain"]) else { return false };
    let want = normal(tree);
    list.lines().filter_map(|line| line.strip_prefix("worktree ")).any(|path| normal(Path::new(path)) == want)
}

fn normal(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) { text.to_ascii_lowercase() } else { text }
}

/// Remove a mutant tree: its links first, then `git worktree remove --force`, then a delete that
/// clears read-only files and retries (Windows refuses to delete a read-only file, and a process
/// that just exited can hold a handle for a moment), then `git worktree prune`.
pub fn remove_tree(repo: &Path, tree: &Path, git_tree: bool) -> bool {
    crate::worktree::unlink_links(tree);
    if git_tree {
        let _ = git(repo, &["worktree", "remove", "--force", &tree.display().to_string()]);
    }
    let gone = remove_dir_robust(tree);
    if git_tree {
        let _ = git(repo, &["worktree", "prune"]);
    }
    gone
}

/// `remove_dir_all`, after clearing read-only bits, up to six times with a growing pause (about
/// 1.5 s in all). What is still held after that is left for the next sweep.
pub fn remove_dir_robust(path: &Path) -> bool {
    if fs::symlink_metadata(path).is_err() {
        return true;
    }
    let mut pause = Duration::from_millis(50);
    for attempt in 0..6 {
        crate::worktree::unlink_links(path);
        clear_readonly_tree(path);
        if fs::remove_dir_all(path).is_ok() || fs::symlink_metadata(path).is_err() {
            return true;
        }
        if attempt < 5 {
            std::thread::sleep(pause);
            pause *= 2;
        }
    }
    fs::symlink_metadata(path).is_err()
}

fn clear_readonly_tree(dir: &Path) {
    clear_readonly(dir);
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            clear_readonly_tree(&entry.path());
        } else {
            clear_readonly(&entry.path());
        }
    }
}

/// Remove what a crashed or cancelled run left: trees of this checkout's slots whose pid is gone,
/// caches of other checkouts unused for two weeks, and 0.7 temp worktrees, copies, and target
/// directories (`probatio-<hex>-<n>`, `probatio-copy-*`, `probatio-cargo-*`). Returns what went.
pub fn sweep(source: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let root = cache_root();
    let own = key_for(source);
    if let Ok(keys) = fs::read_dir(&root) {
        for key in keys.flatten() {
            let key_dir = key.path();
            let name = key.file_name().to_string_lossy().into_owned();
            // Only checkout keys (16 hex digits); `jobs/` and anything else is left alone.
            if !key_dir.is_dir() || name.len() != 16 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                continue;
            }
            let mine = key.file_name().to_string_lossy() == own;
            let recorded = fs::read_to_string(key_dir.join("source.txt")).ok().map(PathBuf::from);
            let repo = if mine { Some(source.to_path_buf()) } else { recorded.clone() };
            let mut live = false;
            // A checkout's folder that has no slot stamp yet is as new as the folder itself.
            let mut newest = fs::metadata(&key_dir).and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let Ok(slots) = fs::read_dir(&key_dir) else { continue };
            for slot in slots.flatten() {
                let dir = slot.path();
                if !dir.is_dir() {
                    continue;
                }
                // Held while this slot is cleaned, so no run starts in it meanwhile.
                let Some(_held) = take_lock(&dir) else {
                    live = true;
                    continue;
                };
                if let Ok(used) = fs::metadata(dir.join("used")).and_then(|meta| meta.modified()) {
                    newest = newest.max(used);
                }
                let trees = fs::read_dir(&dir)
                    .map(|entries| entries.flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("tree")).map(|entry| entry.path()).collect::<Vec<_>>())
                    .unwrap_or_default();
                for tree in trees {
                    let git_tree = tree.join(".git").exists();
                    let gone = match &repo {
                        Some(repo) if repo.is_dir() => remove_tree(repo, &tree, git_tree),
                        _ => remove_dir_robust(&tree),
                    };
                    if gone {
                        removed.push(tree.display().to_string());
                    }
                }
            }
            let source_gone = recorded.as_ref().is_some_and(|path| !path.exists());
            let old = SystemTime::now().duration_since(newest).unwrap_or_default() > STALE_CACHE;
            if !mine && !live && (source_gone || old) && remove_dir_robust(&key_dir) {
                removed.push(key_dir.display().to_string());
            }
        }
    }
    removed.extend(sweep_legacy(&std::env::temp_dir()));
    removed
}

/// 0.7 left `probatio-<16 hex>-<n>` worktrees, `probatio-copy-<16 hex>-m<12 hex>` copies, and
/// `probatio-cargo-<16 hex>-<n>` target directories in the temp folder when it was killed.
fn sweep_legacy(temp: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(temp) else { return removed };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(kind) = legacy_kind(&name) else { continue };
        let path = entry.path();
        let age = fs::metadata(&path).and_then(|meta| meta.modified()).ok().and_then(|when| SystemTime::now().duration_since(when).ok());
        if age.is_none_or(|age| age < STALE_LEGACY) {
            continue;
        }
        let common = if kind == Legacy::Worktree { common_dir_of(&path) } else { None };
        if remove_dir_robust(&path) {
            if let Some(common) = common {
                let _ = Command::new("git")
                    .arg("--git-dir")
                    .arg(&common)
                    .args(["worktree", "prune"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            removed.push(path.display().to_string());
        }
    }
    removed
}

#[derive(PartialEq, Eq, Debug)]
enum Legacy {
    Worktree,
    Copy,
    Target,
}

fn legacy_kind(name: &str) -> Option<Legacy> {
    let hex = |text: &str, len: usize| text.len() == len && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let rest = name.strip_prefix("probatio-")?;
    if let Some(rest) = rest.strip_prefix("cargo-") {
        let (id, slot) = rest.rsplit_once('-')?;
        return (hex(id, 16) && digits(slot)).then_some(Legacy::Target);
    }
    if let Some(rest) = rest.strip_prefix("copy-") {
        let (id, mutant) = rest.split_once('-')?;
        return (hex(id, 16) && mutant.starts_with('m') && hex(&mutant[1..], 12)).then_some(Legacy::Copy);
    }
    let (id, slot) = rest.rsplit_once('-')?;
    (hex(id, 16) && digits(slot)).then_some(Legacy::Worktree)
}

/// A worktree's `.git` file says `gitdir: <common>/worktrees/<name>`.
fn common_dir_of(tree: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(tree.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let common = gitdir.parent()?.parent()?.to_path_buf();
    common.is_dir().then_some(common)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git did not start: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_names_are_known_and_others_are_not() {
        assert_eq!(legacy_kind("probatio-aeb1b487cedf6ca1-0"), Some(Legacy::Worktree));
        assert_eq!(legacy_kind("probatio-cargo-9b4927f54689f310-0"), Some(Legacy::Target));
        assert_eq!(legacy_kind("probatio-copy-9b4927f54689f310-m0123456789ab"), Some(Legacy::Copy));
        // A test's own folder is never taken for a run's leftover, whatever its age.
        for name in ["probatio-test-cli-tokens-10104-3", "probatio-test-timeout-2976-43", "probatio-cli-tokens-10104", "probatio-diff-19824", "probatio-grec-42768-0", "probatio-bench-js", "probatio-cache"] {
            assert_eq!(legacy_kind(name), None, "{name}");
        }
    }

    /// A slot's lock is held while its file is open and free once it is closed, which is what
    /// happens when a run ends or is killed.
    #[test]
    fn a_held_slot_is_skipped_and_a_released_one_is_taken() {
        let dir = crate::testdir::TempDir::new("lock-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let held = take_lock(&dir).expect("a free slot");
        assert!(take_lock(&dir).is_none(), "a held slot was taken twice");
        drop(held);
        assert!(take_lock(&dir).is_some(), "a released slot was not free");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_only_tree_is_removed() {
        let dir = crate::testdir::TempDir::new("ro-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a/b")).unwrap();
        let file = dir.join("a/b/locked.txt");
        fs::write(&file, "x").unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();
        assert!(remove_dir_robust(&dir));
        assert!(!dir.exists());
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn mtime(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    /// The tree keeps your files' mtimes, so cargo finds unchanged crates fresh in the kept target
    /// directory; a file a mutant edited is restored with the current time, and stays newer in
    /// every later tree, so a mutated build is never reused. Ignored files are not copied, what a
    /// suite adds is removed, and the tree goes with the slot.
    #[test]
    fn a_tree_keeps_mtimes_restores_mutated_files_and_goes_with_its_slot() {
        let repo = crate::testdir::TempDir::new("slot-repo");
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::create_dir_all(repo.join("big")).unwrap();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "core.autocrlf", "false"]);
        fs::write(repo.join(".gitignore"), "big/\n").unwrap();
        fs::write(repo.join("src/a.rs"), "one\n").unwrap();
        fs::write(repo.join("keep.txt"), "keep\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("src/a.rs", repo.join("link.rs")).unwrap();
        git_ok(&repo, &["add", "."]);
        git_ok(&repo, &["-c", "user.email=a@b.c", "-c", "user.name=a", "commit", "-q", "-m", "init"]);
        fs::write(repo.join("big/cache.bin"), "ignored\n").unwrap();
        fs::write(repo.join("new.rs"), "untracked\n").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        set_mtime(&repo.join("src/a.rs"), old);
        set_mtime(&repo.join("keep.txt"), old);

        let mut slots = acquire(&repo, true, 1).unwrap();
        let slot = &mut slots[0];
        slot.prepare(&[]).unwrap();
        let tree = slot.tree.clone();
        assert_eq!(fs::read_to_string(tree.join("src/a.rs")).unwrap(), "one\n");
        assert_eq!(mtime(&tree.join("src/a.rs")), old);
        assert!(tree.join("new.rs").is_file() && tree.join(".git").exists());
        assert!(!tree.join("big").exists(), "an ignored folder was copied");
        #[cfg(unix)]
        assert_eq!(fs::read_link(tree.join("link.rs")).unwrap(), Path::new("src/a.rs"), "a tracked symlink is a link in the tree");

        slot.mark_dirty("src/a.rs");
        fs::write(tree.join("src/a.rs"), "MUTATED\n").unwrap();
        fs::write(tree.join("added-by-suite.txt"), "x").unwrap();
        slot.refresh().unwrap();
        assert_eq!(fs::read_to_string(tree.join("src/a.rs")).unwrap(), "one\n");
        assert!(mtime(&tree.join("src/a.rs")) > old);
        assert!(!tree.join("added-by-suite.txt").exists());
        drop(slots);
        assert!(!tree.exists(), "the tree outlived its slot");
        assert!(!registered(&repo, &tree));

        let mut again = acquire(&repo, true, 1).unwrap();
        again[0].prepare(&["big".to_string()]).unwrap();
        assert!(again[0].tree.join("big/cache.bin").is_file(), "a folder named in copy was not copied");
        let tree = again[0].tree.clone();
        assert!(mtime(&tree.join("src/a.rs")) > old, "a file a mutant edited kept its old mtime");
        assert_eq!(mtime(&tree.join("keep.txt")), old);
        drop(again);
        let _ = fs::remove_dir_all(&repo);
    }
}
