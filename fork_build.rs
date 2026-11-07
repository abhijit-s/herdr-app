// Fork only. Included by build.rs, which sets HERDR_FORK_BUILD from it, and
// by src/fork_build.rs's tests.

/// `fork.<short sha>`, with `.dirty` when tracked files differ from HEAD.
/// None outside a git checkout (a tarball) or without git.
pub fn fork_build(dir: &std::path::Path) -> Option<String> {
    let sha = git(dir, &["rev-parse", "--short=7", "HEAD"])?;
    let dirty = !git(dir, &["status", "--porcelain", "--untracked-files=no", "--ignore-submodules=untracked"])?.is_empty();
    Some(format!("fork.{sha}{}", if dirty { ".dirty" } else { "" }))
}

/// What changes when HEAD moves: HEAD, its reflog (written on every commit,
/// checkout and reset), the branch it points to and packed refs (a worktree
/// has its own HEAD and reflog, and shares refs with the main checkout).
/// Not the index: editors and `git status` rewrite it, and each rewrite would
/// recompile the crate.
pub fn git_watch(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let (Some(own), Some(common)) = (
        git(dir, &["rev-parse", "--absolute-git-dir"]),
        git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"]),
    ) else {
        return Vec::new();
    };
    let (own, common) = (std::path::PathBuf::from(own), std::path::PathBuf::from(common));
    let mut files = vec![own.join("HEAD"), own.join("logs/HEAD"), common.join("packed-refs")];
    if let Some(branch) = git(dir, &["symbolic-ref", "-q", "HEAD"]).map(|r| common.join(r)) {
        // A packed branch has no file of its own until its next commit writes one.
        files.push(if branch.exists() { branch } else { branch.parent().unwrap().to_owned() });
    }
    // Cargo reruns every build for a path that does not exist.
    files.retain(|f| f.exists());
    files
}

fn git(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    // A git hook's GIT_DIR or GIT_INDEX_FILE would point it at another tree.
    let out = std::process::Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
