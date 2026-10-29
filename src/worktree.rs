//! Resume removes leftover probatio worktrees for this run before adding a new one.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn worktree_name(run_id: &str) -> String {
    format!("probatio-{run_id}-0")
}

pub fn leftover_paths(list_porcelain: &str, run_id: &str) -> Vec<String> {
    let needle = worktree_name(run_id);
    let mut paths = Vec::new();
    for line in list_porcelain.lines() {
        let Some(path) = line.strip_prefix("worktree ") else { continue };
        if path.contains(&needle) {
            paths.push(path.to_string());
        }
    }
    paths
}

pub fn remove_leftovers(repo: &Path, run_id: &str) -> Result<Vec<String>, String> {
    let list = git(repo, &["worktree", "list", "--porcelain"])?;
    let paths = leftover_paths(&list, run_id);
    for path in &paths {
        let remove = git(repo, &["worktree", "remove", "--force", path]);
        if remove.is_err() {
            let _ = git(repo, &["worktree", "remove", "--force", path]);
        }
    }
    if !paths.is_empty() {
        let _ = git(repo, &["worktree", "prune"]);
    }
    Ok(paths)
}

pub fn add_worktree(repo: &Path, dest: &Path, run_id: &str) -> Result<(), String> {
    remove_leftovers(repo, run_id)?;
    if dest.exists() {
        let _ = std::fs::remove_dir_all(dest);
    }
    git(repo, &["worktree", "add", "--detach", &dest.display().to_string(), "HEAD"])?;
    Ok(())
}

pub fn remove_worktree(repo: &Path, dest: &Path) {
    let _ = git(repo, &["worktree", "remove", "--force", &dest.display().to_string()]);
    let _ = git(repo, &["worktree", "prune"]);
}

pub fn is_git_repo(path: &Path) -> bool {
    git(path, &["rev-parse", "--is-inside-work-tree"]).ok().as_deref() == Some("true\n")
        || git(path, &["rev-parse", "--is-inside-work-tree"]).ok().as_deref() == Some("true")
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn run_id_for(out_dir: &Path) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in out_dir.display().to_string().bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:08x}")
}

pub fn worktree_dest(run_id: &str) -> PathBuf {
    std::env::temp_dir().join(worktree_name(run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn resume_removes_a_leftover_probatio_worktree() {
        let root = std::env::temp_dir().join(format!("probatio-wt-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init"]);
        fs::write(root.join("keep.txt"), "one\n").unwrap();
        git_ok(&root, &["add", "keep.txt"]);
        git_identity(&root, &["commit", "-m", "init"]);
        let run_id = "abc123";
        let dest = std::env::temp_dir().join(worktree_name(run_id));
        let _ = fs::remove_dir_all(&dest);
        git_ok(&root, &["worktree", "add", "--detach", &dest.display().to_string(), "HEAD"]);
        assert!(dest.join("keep.txt").exists());
        let removed = remove_leftovers(&root, run_id).unwrap();
        assert!(removed.iter().any(|path| path.contains(&worktree_name(run_id))));
        assert!(!dest.exists());
        let listed = git(&root, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(leftover_paths(&listed, run_id).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        git(repo, args).unwrap_or_else(|error| panic!("{error}"));
    }

    fn git_identity(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("-c")
            .arg("user.email=probatio@example.com")
            .arg("-c")
            .arg("user.name=probatio")
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
