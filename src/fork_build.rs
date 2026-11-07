//! Which fork commit this Herdr was built from, beside the version. The
//! version stays upstream's: updates, telemetry and layouts compare it.

/// `fork.<short sha>`, `.dirty` when built from uncommitted changes; None in a
/// build without git. Set by build.rs.
pub const BUILD: Option<&str> = option_env!("HERDR_FORK_BUILD");

/// What `--version` prints after the program's name: `0.1.13 (fork 0990622)`.
pub fn version_line(version: &str, build: Option<&str>) -> String {
    match build {
        Some(build) => format!("{version} ({})", build.replacen('.', " ", 1)),
        None => version.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod script {
        include!("../fork_build.rs");
    }

    fn well_formed(build: &str) -> bool {
        let rest = build.strip_prefix("fork.").unwrap_or_default();
        let sha = rest.strip_suffix(".dirty").unwrap_or(rest);
        sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn version_line_names_the_fork_commit() {
        assert_eq!(version_line("0.1.18", Some("fork.8cc5ec1")), "0.1.18 (fork 8cc5ec1)");
        assert_eq!(version_line("0.1.18", Some("fork.8cc5ec1.dirty")), "0.1.18 (fork 8cc5ec1.dirty)");
        assert_eq!(version_line("0.1.18", None), "0.1.18");
    }

    #[test]
    fn the_cores_version_and_build_stay_apart() {
        let core = crate::update::CoreInfo { version: Some("0.1.18".into()), build: Some("fork.8cc5ec1".into()), ..Default::default() };
        let shown = serde_json::to_value(core).unwrap();
        assert_eq!((&shown["version"], &shown["build"]), (&serde_json::json!("0.1.18"), &serde_json::json!("fork.8cc5ec1")));
    }

    #[test]
    fn this_build_is_well_formed() {
        assert!(BUILD.is_none_or(well_formed), "{BUILD:?}");
        assert!(well_formed("fork.8cc5ec1") && well_formed("fork.8cc5ec1.dirty"));
        assert!(!well_formed("fork.") && !well_formed("8cc5ec1") && !well_formed("fork.8CC5EC1") && !well_formed("fork.8cc5"));
    }

    #[test]
    fn build_follows_head_and_the_tree() {
        let dir = std::env::temp_dir().join(format!("herdr-fork-build-{}-{:?}", std::process::id(), std::time::SystemTime::now()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(script::fork_build(&dir), None, "no git: no build");
        assert!(script::git_watch(&dir).is_empty());

        git(&dir, &["init", "-q", "--template="]);
        std::fs::write(dir.join("a"), "1").unwrap();
        git(&dir, &["add", "a"]);
        git(&dir, &["commit", "-qm", "one"]);
        let sha = git(&dir, &["rev-parse", "--short=7", "HEAD"]);
        assert_eq!(script::fork_build(&dir), Some(format!("fork.{sha}")));
        assert!(script::git_watch(&dir).iter().any(|f| f.ends_with(".git/HEAD")));

        std::fs::write(dir.join("untracked"), "").unwrap();
        assert_eq!(script::fork_build(&dir), Some(format!("fork.{sha}")), "untracked files leave it clean");
        std::fs::write(dir.join("a"), "2").unwrap();
        assert_eq!(script::fork_build(&dir), Some(format!("fork.{sha}.dirty")));
        assert!(well_formed(&script::fork_build(&dir).unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
