//! Shared fixtures for integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// A temporary multi-repo workspace with `poly.toml` and two git repos.
pub struct WorkspaceFixture {
    pub root: TempDir,
}

impl WorkspaceFixture {
    /// Layout:
    /// ```text
    /// root/
    ///   poly.toml
    ///   docs/PLATFORM.md
    ///   api/   (git, main, clean)
    ///   web/   (git, main, one unstaged file optional)
    /// ```
    pub fn basic() -> Self {
        let root = TempDir::new().expect("tempdir");
        let root_path = root.path();

        fs::create_dir_all(root_path.join("docs")).unwrap();
        fs::write(
            root_path.join("docs/PLATFORM.md"),
            "# Platform\n\nShared always-doc for agents.\n",
        )
        .unwrap();

        init_git_repo(&root_path.join("api"), "api README\n# API service\n");
        init_git_repo(
            &root_path.join("web"),
            "web README\n# Web frontend oauth login\n",
        );

        // AGENTS files for ranking / context
        fs::write(
            root_path.join("api/AGENTS.md"),
            "# API agents\nUse PYTHONPATH. payments and identity live here.\n",
        )
        .unwrap();
        fs::write(
            root_path.join("web/AGENTS.md"),
            "# Web agents\nNext.js app. oauth login UI.\n",
        )
        .unwrap();
        // Commit AGENTS into git so status is clean unless we dirty later
        git(&root_path.join("api"), &["add", "AGENTS.md"]);
        git(&root_path.join("api"), &["commit", "-m", "agents"]);
        git(&root_path.join("web"), &["add", "AGENTS.md"]);
        git(&root_path.join("web"), &["commit", "-m", "agents"]);

        fs::write(
            root_path.join("poly.toml"),
            r#"
schema_version = 1

[workspace]
name = "fixture"

[context]
always = ["docs/PLATFORM.md"]
max_chars = 20000

[[repos]]
id = "api"
path = "api"
role = "api"
tags = ["backend", "payments", "identity", "oauth"]
description = "HTTP API"

[[repos]]
id = "web"
path = "web"
role = "frontend"
tags = ["frontend", "oauth", "login"]
depends_on = ["api"]
description = "Web app"
"#,
        )
        .unwrap();

        Self { root }
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn config(&self) -> PathBuf {
        self.path().join("poly.toml")
    }

    /// Create an unstaged change in `web`.
    pub fn dirty_web(&self) {
        fs::write(self.path().join("web/DIRTY.txt"), "dirty\n").unwrap();
    }
}

fn init_git_repo(path: &Path, readme: &str) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.email", "poly-test@example.com"]);
    git(path, &["config", "user.name", "poly test"]);
    // Avoid parent git hooks / signing issues in CI
    git(path, &["config", "commit.gpgsign", "false"]);
    fs::write(path.join("README.md"), readme).unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-m", "initial"]);
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} in {}: {e}", args, cwd.display()));
    if !out.status.success() {
        panic!(
            "git {:?} failed in {}:\n{}\n{}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
