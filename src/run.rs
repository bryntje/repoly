use crate::config::{RepoEntry, Workspace};
use anyhow::{bail, Context, Result};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Result of running a command in one repo.
#[derive(Debug)]
pub struct RepoRunResult {
    pub repo_id: String,
    pub path: String,
    pub status: Option<ExitStatus>,
    pub error: Option<String>,
    /// Captured stdout (when using capture mode / MCP).
    pub stdout: Option<String>,
    /// Captured stderr (when using capture mode / MCP).
    pub stderr: Option<String>,
    /// Child was killed due to timeout.
    pub timed_out: bool,
    /// stdout was truncated to max_output_bytes.
    pub stdout_truncated: bool,
    /// stderr was truncated to max_output_bytes.
    pub stderr_truncated: bool,
}

impl RepoRunResult {
    pub fn success(&self) -> bool {
        !self.timed_out && self.error.is_none() && self.status.map(|s| s.success()).unwrap_or(false)
    }

    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }

    fn empty_fail(repo_id: String, path: String, error: String) -> Self {
        Self {
            repo_id,
            path,
            status: None,
            error: Some(error),
            stdout: None,
            stderr: None,
            timed_out: false,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }
}

/// Limits for captured child processes (MCP / non-interactive).
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureOpts {
    /// Kill the child after this many seconds (None = no limit).
    pub timeout_secs: Option<u64>,
    /// Max bytes kept per stream (None = unlimited).
    pub max_output_bytes: Option<usize>,
}

/// Resolve which repos to target for `run`.
pub fn select_repos<'a>(
    workspace: &'a Workspace,
    repos: Option<&[String]>,
    tags: Option<&[String]>,
    role: Option<&str>,
) -> Result<Vec<&'a RepoEntry>> {
    // Require an explicit filter so `repoly run -- rm` cannot hit every root by accident.
    if repos.is_none() && tags.is_none() && role.is_none() {
        bail!("repoly run requires --repos, --tags, and/or --role (refusing to target all repos)");
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

/// How to launch a child process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LaunchMode {
    /// `program arg1 arg2` — no shell (default, safer).
    #[default]
    Argv,
    /// `sh -c "…"` (Unix) / `cmd /C "…"` (Windows). Needs explicit `--shell`.
    Shell,
}

impl LaunchMode {
    pub fn from_shell_flag(shell: bool) -> Self {
        if shell {
            LaunchMode::Shell
        } else {
            LaunchMode::Argv
        }
    }
}

/// Run `cmd` in a single repo. Inherits stdio (interactive tools work).
pub fn exec_one(
    workspace: &Workspace,
    repo: &RepoEntry,
    cmd: &[String],
    dry_run: bool,
    mode: LaunchMode,
) -> Result<RepoRunResult> {
    if cmd.is_empty() {
        bail!("no command specified (usage: repoly exec <repo> -- <command...>)");
    }

    let path = workspace.repo_path(repo);
    if !path.exists() {
        return Ok(RepoRunResult::empty_fail(
            repo.id.clone(),
            path.display().to_string(),
            "path does not exist".into(),
        ));
    }

    if dry_run {
        eprintln!(
            "[dry-run] {} @ {}: {}",
            repo.id,
            path.display(),
            format_cmd(cmd, mode)
        );
        return Ok(RepoRunResult {
            repo_id: repo.id.clone(),
            path: path.display().to_string(),
            status: Some(dummy_success()),
            error: None,
            stdout: None,
            stderr: None,
            timed_out: false,
            stdout_truncated: false,
            stderr_truncated: false,
        });
    }

    let status = spawn_inherit(workspace, repo, &path, cmd, mode)?;
    Ok(RepoRunResult {
        repo_id: repo.id.clone(),
        path: path.display().to_string(),
        status: Some(status),
        error: None,
        stdout: None,
        stderr: None,
        timed_out: false,
        stdout_truncated: false,
        stderr_truncated: false,
    })
}

/// Run `cmd` capturing stdout/stderr (for MCP / non-interactive callers).
pub fn exec_capture(
    workspace: &Workspace,
    repo: &RepoEntry,
    cmd: &[String],
    mode: LaunchMode,
) -> Result<RepoRunResult> {
    exec_capture_opts(workspace, repo, cmd, mode, CaptureOpts::default())
}

/// Like [`exec_capture`] with optional timeout and output caps.
pub fn exec_capture_opts(
    workspace: &Workspace,
    repo: &RepoEntry,
    cmd: &[String],
    mode: LaunchMode,
    opts: CaptureOpts,
) -> Result<RepoRunResult> {
    if cmd.is_empty() {
        bail!("no command specified");
    }
    let path = workspace.repo_path(repo);
    if !path.exists() {
        return Ok(RepoRunResult::empty_fail(
            repo.id.clone(),
            path.display().to_string(),
            "path does not exist".into(),
        ));
    }
    match spawn_capture(
        &workspace.name,
        &workspace.root,
        repo,
        &path,
        cmd,
        mode,
        opts,
    ) {
        Ok(captured) => Ok(RepoRunResult {
            repo_id: repo.id.clone(),
            path: path.display().to_string(),
            status: Some(captured.status),
            error: if captured.timed_out {
                Some(format!(
                    "command timed out after {}s",
                    opts.timeout_secs.unwrap_or(0)
                ))
            } else {
                None
            },
            stdout: Some(captured.stdout),
            stderr: Some(captured.stderr),
            timed_out: captured.timed_out,
            stdout_truncated: captured.stdout_truncated,
            stderr_truncated: captured.stderr_truncated,
        }),
        Err(e) => Ok(RepoRunResult::empty_fail(
            repo.id.clone(),
            path.display().to_string(),
            e.to_string(),
        )),
    }
}

/// Run `cmd` across repos sequentially (inherit stdio) or in parallel (captured).
pub fn run_many(
    workspace: &Workspace,
    repos: &[&RepoEntry],
    cmd: &[String],
    parallel: bool,
    continue_on_error: bool,
    dry_run: bool,
    mode: LaunchMode,
) -> Result<Vec<RepoRunResult>> {
    if cmd.is_empty() {
        bail!("no command specified (usage: repoly run --repos a,b -- <command...>)");
    }

    if parallel && repos.len() > 1 {
        return run_parallel(workspace, repos, cmd, dry_run, mode);
    }

    let mut results = Vec::new();
    for repo in repos {
        print_banner(&repo.id, &workspace.repo_path(repo), cmd, mode);
        let r = exec_one(workspace, repo, cmd, dry_run, mode)?;
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
    mode: LaunchMode,
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
                    format_cmd(&cmd, mode)
                );
                RepoRunResult {
                    repo_id: repo.id.clone(),
                    path: path.display().to_string(),
                    status: Some(dummy_success()),
                    error: None,
                    stdout: None,
                    stderr: None,
                    timed_out: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                }
            } else if !path.exists() {
                RepoRunResult::empty_fail(
                    repo.id.clone(),
                    path.display().to_string(),
                    "path does not exist".into(),
                )
            } else {
                match spawn_capture(
                    &ws_name,
                    &root,
                    &repo,
                    &path,
                    &cmd,
                    mode,
                    CaptureOpts::default(),
                ) {
                    Ok(captured) => {
                        let _ = tx.send((
                            repo.id.clone(),
                            captured.stdout.clone(),
                            captured.stderr.clone(),
                        ));
                        RepoRunResult {
                            repo_id: repo.id.clone(),
                            path: path.display().to_string(),
                            status: Some(captured.status),
                            error: None,
                            stdout: Some(captured.stdout),
                            stderr: Some(captured.stderr),
                            timed_out: captured.timed_out,
                            stdout_truncated: captured.stdout_truncated,
                            stderr_truncated: captured.stderr_truncated,
                        }
                    }
                    Err(e) => RepoRunResult::empty_fail(
                        repo.id.clone(),
                        path.display().to_string(),
                        e.to_string(),
                    ),
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

fn print_banner(id: &str, path: &Path, cmd: &[String], mode: LaunchMode) {
    eprintln!("\n══ {} ══", id);
    eprintln!("path: {}", path.display());
    eprintln!("$ {}", format_cmd(cmd, mode));
}

fn inject_env(cmd: &mut Command, workspace_name: &str, root: &Path, repo: &RepoEntry, path: &Path) {
    cmd.env("REPOLY_WORKSPACE", workspace_name);
    cmd.env("REPOLY_ROOT", root);
    cmd.env("REPOLY_REPO", &repo.id);
    cmd.env("REPOLY_REPO_PATH", path);
    if let Some(role) = &repo.role {
        cmd.env("REPOLY_REPO_ROLE", role);
    }
}

fn spawn_inherit(
    workspace: &Workspace,
    repo: &RepoEntry,
    path: &Path,
    cmd: &[String],
    mode: LaunchMode,
) -> Result<ExitStatus> {
    let mut c = build_command(cmd, mode)?;
    c.current_dir(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    inject_env(&mut c, &workspace.name, &workspace.root, repo, path);
    c.status()
        .with_context(|| format!("failed to spawn command in {}", path.display()))
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    timed_out: bool,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

fn spawn_capture(
    workspace_name: &str,
    root: &Path,
    repo: &RepoEntry,
    path: &Path,
    cmd: &[String],
    mode: LaunchMode,
    opts: CaptureOpts,
) -> Result<CapturedOutput> {
    let mut c = build_command(cmd, mode)?;
    c.current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    inject_env(&mut c, workspace_name, root, repo, path);
    let mut child = c
        .spawn()
        .with_context(|| format!("failed to spawn command in {}", path.display()))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let max = opts.max_output_bytes;
    let out_handle = thread::spawn(move || read_capped(stdout_pipe.take(), max));
    let err_handle = thread::spawn(move || read_capped(stderr_pipe.take(), max));

    let deadline = opts
        .timeout_secs
        .map(|s| Instant::now() + Duration::from_secs(s));

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if let Some(dl) = deadline {
                    if Instant::now() >= dl {
                        timed_out = true;
                        let _ = child.kill();
                        break child.wait().unwrap_or_else(|_| dummy_success());
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return Err(e).context("waiting for child process");
            }
        }
    };

    let (stdout, stdout_truncated) = out_handle.join().unwrap_or_default();
    let (stderr, stderr_truncated) = err_handle.join().unwrap_or_default();

    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
        timed_out,
        stdout_truncated,
        stderr_truncated,
    })
}

fn read_capped(pipe: Option<impl Read>, max: Option<usize>) -> (String, bool) {
    let Some(mut pipe) = pipe else {
        return (String::new(), false);
    };
    let mut buf = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Some(limit) = max {
                    if buf.len() >= limit {
                        truncated = true;
                        // Drain remainder without storing
                        let mut sink = [0u8; 8192];
                        while let Ok(m) = pipe.read(&mut sink) {
                            if m == 0 {
                                break;
                            }
                        }
                        break;
                    }
                    let room = limit.saturating_sub(buf.len());
                    let take = n.min(room);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                        let mut sink = [0u8; 8192];
                        while let Ok(m) = pipe.read(&mut sink) {
                            if m == 0 {
                                break;
                            }
                        }
                        break;
                    }
                } else {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
            Err(_) => break,
        }
    }
    if truncated {
        const MARKER: &str = "\n… [truncated by repoly; raise --exec-max-output-bytes]\n";
        let marker_bytes = MARKER.as_bytes();
        if let Some(limit) = max {
            if buf.len() + marker_bytes.len() > limit && buf.len() > marker_bytes.len() {
                buf.truncate(limit.saturating_sub(marker_bytes.len()));
            }
        }
        buf.extend_from_slice(marker_bytes);
    }
    (String::from_utf8_lossy(&buf).into_owned(), truncated)
}

fn build_command(cmd: &[String], mode: LaunchMode) -> Result<Command> {
    match mode {
        LaunchMode::Argv => {
            let (program, args) = split_cmd(cmd)?;
            let mut c = Command::new(program);
            c.args(args);
            Ok(c)
        }
        LaunchMode::Shell => {
            let line = shell_script(cmd);
            Ok(shell_command(&line))
        }
    }
}

/// Build platform shell invocation for a script line.
fn shell_command(script: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(script);
        c
    }
}

/// Script string for shell mode: single arg used as-is; else shell-quoted join.
fn shell_script(cmd: &[String]) -> String {
    if cmd.len() == 1 {
        cmd[0].clone()
    } else {
        shell_join(cmd)
    }
}

fn format_cmd(cmd: &[String], mode: LaunchMode) -> String {
    match mode {
        LaunchMode::Argv => shell_join(cmd),
        LaunchMode::Shell => {
            #[cfg(unix)]
            {
                format!("sh -c {}", shell_quote(&shell_script(cmd)))
            }
            #[cfg(windows)]
            {
                format!("cmd /C {}", shell_quote(&shell_script(cmd)))
            }
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
        eprintln!("  [{mark}] {}  exit={code}{err}  ({})", r.repo_id, r.path);
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
            config_path: PathBuf::from("/tmp/t/repoly.toml"),
            context: ContextSection::default(),
            policy: Default::default(),
            ranking: Default::default(),
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

    #[test]
    fn shell_script_single_arg_passthrough() {
        assert_eq!(
            shell_script(&["npm test && echo ok".into()]),
            "npm test && echo ok"
        );
    }

    #[test]
    fn format_cmd_marks_shell() {
        let s = format_cmd(&["echo hi".into()], LaunchMode::Shell);
        assert!(s.contains("sh -c") || s.contains("cmd /C"));
    }
}
