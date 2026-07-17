use crate::config::{RepoEntry, Workspace};
use anyhow::{bail, Context, Result};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;

/// Result of running a command in one repo.
#[derive(Debug)]
pub struct RepoRunResult {
    pub repo_id: String,
    pub path: String,
    pub status: Option<ExitStatus>,
    pub error: Option<String>,
}

impl RepoRunResult {
    pub fn success(&self) -> bool {
        self.error.is_none() && self.status.map(|s| s.success()).unwrap_or(false)
    }

    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }
}

/// Resolve which repos to target for `run`.
pub fn select_repos<'a>(
    workspace: &'a Workspace,
    repos: Option<&[String]>,
    tags: Option<&[String]>,
    role: Option<&str>,
) -> Result<Vec<&'a RepoEntry>> {
    // Require an explicit filter so `poly run -- rm` cannot hit every root by accident.
    if repos.is_none() && tags.is_none() && role.is_none() {
        bail!("poly run requires --repos, --tags, and/or --role (refusing to target all repos)");
    }

    let mut set: Vec<&RepoEntry> = workspace.repos.iter().collect();

    if let Some(ids) = repos {
        for id in ids {
            if workspace.repos.iter().all(|r| &r.id != id) {
                bail!("unknown repo id '{id}'");
            }
        }
        set.retain(|r| ids.iter().any(|id| id == &r.id));
    }
    if let Some(tags) = tags {
        set.retain(|r| r.tags.iter().any(|t| tags.iter().any(|ft| ft == t)));
    }
    if let Some(role) = role {
        set.retain(|r| r.role.as_deref() == Some(role));
    }

    if set.is_empty() {
        bail!("no repos matched filters; check --repos / --tags / --role");
    }
    Ok(set)
}

pub fn resolve_repo<'a>(workspace: &'a Workspace, id: &str) -> Result<&'a RepoEntry> {
    workspace
        .repos
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown repo id '{id}'"))
}

/// Run `cmd` in a single repo. Inherits stdio (interactive tools work).
pub fn exec_one(
    workspace: &Workspace,
    repo: &RepoEntry,
    cmd: &[String],
    dry_run: bool,
) -> Result<RepoRunResult> {
    if cmd.is_empty() {
        bail!("no command specified (usage: poly exec <repo> -- <command...>)");
    }

    let path = workspace.repo_path(repo);
    if !path.exists() {
        return Ok(RepoRunResult {
            repo_id: repo.id.clone(),
            path: path.display().to_string(),
            status: None,
            error: Some("path does not exist".into()),
        });
    }

    if dry_run {
        eprintln!(
            "[dry-run] {} @ {}: {}",
            repo.id,
            path.display(),
            shell_join(cmd)
        );
        return Ok(RepoRunResult {
            repo_id: repo.id.clone(),
            path: path.display().to_string(),
            status: Some(dummy_success()),
            error: None,
        });
    }

    let status = spawn_inherit(workspace, repo, &path, cmd)?;
    Ok(RepoRunResult {
        repo_id: repo.id.clone(),
        path: path.display().to_string(),
        status: Some(status),
        error: None,
    })
}

/// Run `cmd` across repos sequentially (inherit stdio) or in parallel (captured).
pub fn run_many(
    workspace: &Workspace,
    repos: &[&RepoEntry],
    cmd: &[String],
    parallel: bool,
    continue_on_error: bool,
    dry_run: bool,
) -> Result<Vec<RepoRunResult>> {
    if cmd.is_empty() {
        bail!("no command specified (usage: poly run --repos a,b -- <command...>)");
    }

    if parallel && repos.len() > 1 {
        return run_parallel(workspace, repos, cmd, dry_run);
    }

    let mut results = Vec::new();
    for repo in repos {
        print_banner(&repo.id, &workspace.repo_path(repo), cmd);
        let r = exec_one(workspace, repo, cmd, dry_run)?;
        let ok = r.success();
        results.push(r);
        if !ok && !continue_on_error {
            break;
        }
    }
    Ok(results)
}

fn run_parallel(
    workspace: &Workspace,
    repos: &[&RepoEntry],
    cmd: &[String],
    dry_run: bool,
) -> Result<Vec<RepoRunResult>> {
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();

    for repo in repos {
        let tx = tx.clone();
        let repo = (*repo).clone();
        let cmd = cmd.to_vec();
        let ws_name = workspace.name.clone();
        let root = workspace.root.clone();
        let path = workspace.repo_path(&repo);

        handles.push(thread::spawn(move || {
            let result = if dry_run {
                eprintln!(
                    "[dry-run] {} @ {}: {}",
                    repo.id,
                    path.display(),
                    shell_join(&cmd)
                );
                RepoRunResult {
                    repo_id: repo.id.clone(),
                    path: path.display().to_string(),
                    status: Some(dummy_success()),
                    error: None,
                }
            } else if !path.exists() {
                RepoRunResult {
                    repo_id: repo.id.clone(),
                    path: path.display().to_string(),
                    status: None,
                    error: Some("path does not exist".into()),
                }
            } else {
                match spawn_capture(&ws_name, &root, &repo, &path, &cmd) {
                    Ok((status, stdout, stderr)) => {
                        let _ = tx.send((repo.id.clone(), stdout, stderr));
                        RepoRunResult {
                            repo_id: repo.id.clone(),
                            path: path.display().to_string(),
                            status: Some(status),
                            error: None,
                        }
                    }
                    Err(e) => RepoRunResult {
                        repo_id: repo.id.clone(),
                        path: path.display().to_string(),
                        status: None,
                        error: Some(e.to_string()),
                    },
                }
            };
            result
        }));
    }
    drop(tx);

    // Stream captured output as it arrives
    let stdout = io::stdout();
    let mut out = stdout.lock();
    while let Ok((id, so, se)) = rx.recv() {
        let _ = writeln!(out, "\n── {} ──", id);
        if !so.is_empty() {
            let _ = write!(out, "{so}");
            if !so.ends_with('\n') {
                let _ = writeln!(out);
            }
        }
        if !se.is_empty() {
            let _ = write!(io::stderr(), "{se}");
        }
    }

    let mut results = Vec::new();
    for h in handles {
        match h.join() {
            Ok(r) => results.push(r),
            Err(_) => bail!("worker thread panicked"),
        }
    }
    // Stable order by original repo list
    results.sort_by(|a, b| {
        let ia = repos.iter().position(|r| r.id == a.repo_id).unwrap_or(0);
        let ib = repos.iter().position(|r| r.id == b.repo_id).unwrap_or(0);
        ia.cmp(&ib)
    });
    Ok(results)
}

fn print_banner(id: &str, path: &Path, cmd: &[String]) {
    eprintln!("\n══ {} ══", id);
    eprintln!("path: {}", path.display());
    eprintln!("$ {}", shell_join(cmd));
}

fn inject_env(cmd: &mut Command, workspace_name: &str, root: &Path, repo: &RepoEntry, path: &Path) {
    cmd.env("POLY_WORKSPACE", workspace_name);
    cmd.env("POLY_ROOT", root);
    cmd.env("POLY_REPO", &repo.id);
    cmd.env("POLY_REPO_PATH", path);
    if let Some(role) = &repo.role {
        cmd.env("POLY_REPO_ROLE", role);
    }
}

fn spawn_inherit(
    workspace: &Workspace,
    repo: &RepoEntry,
    path: &Path,
    cmd: &[String],
) -> Result<ExitStatus> {
    let (program, args) = split_cmd(cmd)?;
    let mut c = Command::new(program);
    c.args(args)
        .current_dir(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    inject_env(&mut c, &workspace.name, &workspace.root, repo, path);
    c.status()
        .with_context(|| format!("failed to spawn '{program}' in {}", path.display()))
}

fn spawn_capture(
    workspace_name: &str,
    root: &Path,
    repo: &RepoEntry,
    path: &Path,
    cmd: &[String],
) -> Result<(ExitStatus, String, String)> {
    let (program, args) = split_cmd(cmd)?;
    let mut c = Command::new(program);
    c.args(args)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    inject_env(&mut c, workspace_name, root, repo, path);
    let output = c
        .output()
        .with_context(|| format!("failed to spawn '{program}' in {}", path.display()))?;
    Ok((
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

fn split_cmd(cmd: &[String]) -> Result<(&str, &[String])> {
    let program = cmd
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    Ok((program, &cmd[1..]))
}

fn shell_join(cmd: &[String]) -> String {
    cmd.iter()
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// ExitStatus::success() for dry-run without spawning.
fn dummy_success() -> ExitStatus {
    // Portable trick: run `true` is not ideal offline; use a zero-exit from a known command.
    // On Unix, `std::os::unix::process::ExitStatusExt` — keep simple with actual true.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
    #[cfg(not(unix))]
    {
        Command::new("cmd")
            .args(["/C", "exit", "0"])
            .status()
            .expect("dry-run status")
    }
}

pub fn summarize(results: &[RepoRunResult]) {
    if results.is_empty() {
        return;
    }
    eprintln!("\n── summary ──");
    for r in results {
        let mark = if r.success() { "ok" } else { "FAIL" };
        let code = r
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        let err = r
            .error
            .as_ref()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default();
        eprintln!(
            "  [{mark}] {}  exit={code}{err}  ({})",
            r.repo_id, r.path
        );
    }
}

pub fn exit_code_from_results(results: &[RepoRunResult]) -> u8 {
    if results.is_empty() {
        return 1;
    }
    if results.iter().all(|r| r.success()) {
        0
    } else if results.iter().any(|r| r.success()) {
        2 // partial
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContextSection, RepoEntry, Workspace};
    use std::path::PathBuf;

    fn ws() -> Workspace {
        Workspace {
            name: "t".into(),
            root: PathBuf::from("/tmp/t"),
            config_path: PathBuf::from("/tmp/t/poly.toml"),
            context: ContextSection::default(),
            repos: vec![
                RepoEntry {
                    id: "api".into(),
                    path: "./api".into(),
                    role: Some("api".into()),
                    tags: vec!["backend".into()],
                    depends_on: vec![],
                    description: None,
                    context_files: None,
                },
                RepoEntry {
                    id: "web".into(),
                    path: "./web".into(),
                    role: Some("frontend".into()),
                    tags: vec!["frontend".into()],
                    depends_on: vec![],
                    description: None,
                    context_files: None,
                },
            ],
        }
    }

    #[test]
    fn select_by_repos() {
        let w = ws();
        let ids = vec!["web".into()];
        let s = select_repos(&w, Some(&ids), None, None).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].id, "web");
    }

    #[test]
    fn select_unknown_errors() {
        let w = ws();
        let ids = vec!["nope".into()];
        assert!(select_repos(&w, Some(&ids), None, None).is_err());
    }

    #[test]
    fn shell_join_quotes() {
        assert_eq!(shell_join(&["echo".into(), "a b".into()]), "echo \"a b\"");
    }
}
