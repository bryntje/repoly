//! Safe git commit helper scoped to repoly workspace repos.

use crate::config::{RepoEntry, Workspace};
use crate::policy;
use crate::run;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct CommitOpts {
    pub message: String,
    /// `git add -A` before commit
    pub all: bool,
    /// Only stage these paths (relative to repo root). Mutually exclusive with all in practice.
    pub paths: Vec<String>,
    pub amend: bool,
    pub allow_empty: bool,
    pub no_verify: bool,
    pub dry_run: bool,
    /// Sign-off (-s)
    pub signoff: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitResult {
    pub repo: String,
    pub path: String,
    pub success: bool,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub commit_sha: Option<String>,
    pub error: Option<String>,
}

pub fn commit_one(
    workspace: &Workspace,
    repo: &RepoEntry,
    opts: &CommitOpts,
) -> Result<CommitResult> {
    validate_opts(opts)?;

    let path = workspace.repo_path(repo);
    let mut result = CommitResult {
        repo: repo.id.clone(),
        path: path.display().to_string(),
        success: false,
        skipped: false,
        skip_reason: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        commit_sha: None,
        error: None,
    };

    if !path.exists() {
        result.error = Some("path does not exist".into());
        return Ok(result);
    }
    if !path.join(".git").exists() {
        result.skipped = true;
        result.skip_reason = Some("not a git repository".into());
        result.success = true; // not a failure of the tool for non-git meta folders
        return Ok(result);
    }

    // Stage
    if opts.all {
        if opts.dry_run {
            result.stdout.push_str("[dry-run] git add -A\n");
        } else {
            let (code, so, se) = git(&path, &["add", "-A"])?;
            result.stdout.push_str(&so);
            result.stderr.push_str(&se);
            if code != 0 {
                result.exit_code = Some(code);
                result.error = Some(format!("git add -A failed (exit {code})"));
                return Ok(result);
            }
        }
    } else if !opts.paths.is_empty() {
        // Confine pathspecs to the repo root (always-on layer 2).
        let confined = match confine_pathspecs(&path, &opts.paths) {
            Ok(p) => p,
            Err(e) => {
                result.error = Some(e.to_string());
                return Ok(result);
            }
        };
        if opts.dry_run {
            result
                .stdout
                .push_str(&format!("[dry-run] git add -- {}\n", confined.join(" ")));
        } else {
            let mut args = vec!["add".to_string(), "--".to_string()];
            args.extend(confined);
            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let (code, so, se) = git(&path, &args_ref)?;
            result.stdout.push_str(&so);
            result.stderr.push_str(&se);
            if code != 0 {
                result.exit_code = Some(code);
                result.error = Some(format!("git add failed (exit {code})"));
                return Ok(result);
            }
        }
    }

    // Nothing to commit?
    if !opts.amend && !opts.allow_empty && !opts.dry_run {
        let staged = has_staged_changes(&path)?;
        if !staged {
            result.skipped = true;
            result.skip_reason =
                Some("nothing staged to commit (use --all or stage files first)".into());
            result.success = true;
            return Ok(result);
        }
    }

    // Build commit argv
    let mut args: Vec<String> = vec!["commit".into(), "-m".into(), opts.message.clone()];
    if opts.amend {
        args.push("--amend".into());
    }
    if opts.allow_empty {
        args.push("--allow-empty".into());
    }
    if opts.no_verify {
        args.push("--no-verify".into());
    }
    if opts.signoff {
        args.push("--signoff".into());
    }

    if opts.dry_run {
        result.stdout.push_str(&format!(
            "[dry-run] git {}\n",
            args.iter()
                .map(|a| {
                    if a.contains(' ') {
                        format!("\"{a}\"")
                    } else {
                        a.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        ));
        result.success = true;
        result.exit_code = Some(0);
        return Ok(result);
    }

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (code, so, se) = git(&path, &args_ref)?;
    result.stdout.push_str(&so);
    result.stderr.push_str(&se);
    result.exit_code = Some(code);
    if code != 0 {
        result.error = Some(format!("git commit failed (exit {code})"));
        return Ok(result);
    }

    result.success = true;
    if let Ok((0, sha, _)) = git(&path, &["rev-parse", "HEAD"]) {
        result.commit_sha = Some(sha.trim().to_string());
    }
    Ok(result)
}

pub fn commit_many(
    workspace: &Workspace,
    repos: &[&RepoEntry],
    opts: &CommitOpts,
) -> Result<Vec<CommitResult>> {
    validate_opts(opts)?;
    let mut out = Vec::new();
    for repo in repos {
        out.push(commit_one(workspace, repo, opts)?);
    }
    Ok(out)
}

fn validate_opts(opts: &CommitOpts) -> Result<()> {
    let msg = opts.message.trim();
    if msg.is_empty() {
        bail!("commit message must not be empty (-m / --message)");
    }
    if msg.chars().count() > 10_000 {
        bail!("commit message too long");
    }
    // Block obviously dangerous multi-message tricks via -m only once (we control argv)
    if opts.all && !opts.paths.is_empty() {
        bail!("use either --all or pathspecs, not both");
    }
    Ok(())
}

/// Validate pathspecs stay under `repo_root`; return paths relative to the repo for `git add`.
fn confine_pathspecs(repo_root: &Path, paths: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(paths.len());
    for raw in paths {
        let user = Path::new(raw);
        let abs = policy::ensure_under_root(repo_root, user)?;
        let rel = abs.strip_prefix(
            repo_root
                .canonicalize()
                .unwrap_or_else(|_| repo_root.to_path_buf()),
        );
        let for_git = match rel {
            Ok(r) if !r.as_os_str().is_empty() => r.to_path_buf(),
            Ok(_) => PathBuf::from("."),
            // Fall back to original relative form if strip fails but ensure passed
            Err(_) => {
                if user.is_absolute() {
                    abs.clone()
                } else {
                    user.to_path_buf()
                }
            }
        };
        out.push(for_git.to_string_lossy().into_owned());
    }
    Ok(out)
}

fn has_staged_changes(path: &Path) -> Result<bool> {
    // diff --cached --quiet → exit 1 if differences, 0 if none
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(path)
        .status()
        .context("git diff --cached")?;
    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(code) => bail!("git diff --cached --quiet exited {code}"),
        None => bail!("git diff --cached interrupted"),
    }
}

fn git(cwd: &Path, args: &[&str]) -> Result<(i32, String, String)> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, cwd.display()))?;
    Ok((
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

pub fn print_results(results: &[CommitResult]) {
    eprintln!("\n── commit summary ──");
    for r in results {
        if r.skipped {
            eprintln!(
                "  [skip] {} — {}",
                r.repo,
                r.skip_reason.as_deref().unwrap_or("skipped")
            );
            continue;
        }
        if r.success {
            let sha = r
                .commit_sha
                .as_deref()
                .map(|s| if s.len() > 8 { &s[..8] } else { s })
                .unwrap_or("?");
            eprintln!("  [ok]   {}  {sha}", r.repo);
        } else {
            eprintln!(
                "  [FAIL] {}  {}",
                r.repo,
                r.error.as_deref().unwrap_or("failed")
            );
            if !r.stderr.trim().is_empty() {
                for line in r.stderr.lines().take(5) {
                    eprintln!("         {line}");
                }
            }
        }
    }
}

pub fn exit_code(results: &[CommitResult]) -> u8 {
    if results.is_empty() {
        return 1;
    }
    let any_fail = results.iter().any(|r| !r.success && !r.skipped);
    let any_ok = results.iter().any(|r| r.success);
    if any_fail && any_ok {
        2
    } else if any_fail {
        1
    } else {
        0
    }
}

/// Resolve single-repo or multi-repo targets for commit.
pub fn resolve_targets<'a>(
    workspace: &'a Workspace,
    repo: Option<&str>,
    repos: Option<&[String]>,
    tags: Option<&[String]>,
    role: Option<&str>,
) -> Result<Vec<&'a RepoEntry>> {
    if let Some(id) = repo {
        if repos.is_some() || tags.is_some() || role.is_some() {
            bail!("use either <repo> or --repos/--tags/--role, not both");
        }
        return Ok(vec![run::resolve_repo(workspace, id)?]);
    }
    run::select_repos(workspace, repos, tags, role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_message() {
        let opts = CommitOpts {
            message: "   ".into(),
            all: false,
            paths: vec![],
            amend: false,
            allow_empty: false,
            no_verify: false,
            dry_run: true,
            signoff: false,
        };
        assert!(validate_opts(&opts).is_err());
    }

    #[test]
    fn rejects_all_and_paths() {
        let opts = CommitOpts {
            message: "ok".into(),
            all: true,
            paths: vec!["a".into()],
            amend: false,
            allow_empty: false,
            no_verify: false,
            dry_run: true,
            signoff: false,
        };
        assert!(validate_opts(&opts).is_err());
    }
}
