use crate::config::Workspace;
use chrono::Utc;
use rayon::prelude::*;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub workspace: String,
    pub root: String,
    pub generated_at: String,
    pub repos: Vec<RepoStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoStatus {
    pub id: String,
    pub path: String,
    pub exists: bool,
    pub is_git: bool,
    pub branch: Option<String>,
    pub dirty: bool,
    pub dirty_count: u32,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub head_subject: Option<String>,
    pub error: Option<String>,
}

pub fn collect_status(
    workspace: &Workspace,
    filter: Option<&[String]>,
    fetch: bool,
) -> StatusReport {
    let repos: Vec<_> = workspace
        .repos
        .iter()
        .filter(|r| {
            filter
                .map(|f| f.iter().any(|id| id == &r.id))
                .unwrap_or(true)
        })
        .collect();

    let statuses: Vec<RepoStatus> = repos
        .par_iter()
        .map(|repo| {
            let path = workspace.repo_path(repo);
            status_one(&repo.id, &path, fetch)
        })
        .collect();

    StatusReport {
        workspace: workspace.name.clone(),
        root: workspace.root.display().to_string(),
        generated_at: Utc::now().to_rfc3339(),
        repos: statuses,
    }
}

fn status_one(id: &str, path: &Path, fetch: bool) -> RepoStatus {
    let mut st = RepoStatus {
        id: id.to_string(),
        path: path.display().to_string(),
        exists: path.exists(),
        is_git: false,
        branch: None,
        dirty: false,
        dirty_count: 0,
        upstream: None,
        ahead: None,
        behind: None,
        head_subject: None,
        error: None,
    };

    if !st.exists {
        st.error = Some("path does not exist".into());
        return st;
    }

    // .git can be a dir or a file (worktree)
    let git_marker = path.join(".git");
    if !git_marker.exists() {
        return st;
    }
    st.is_git = true;

    if fetch {
        let _ = git(path, &["fetch", "--quiet"]);
    }

    match git(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(b) => {
            let b = b.trim().to_string();
            if b != "HEAD" {
                st.branch = Some(b);
            } else {
                // detached
                if let Ok(short) = git(path, &["rev-parse", "--short", "HEAD"]) {
                    st.branch = Some(format!("detached@{}", short.trim()));
                }
            }
        }
        Err(e) => {
            st.error = Some(e);
            return st;
        }
    }

    match git(path, &["status", "--porcelain"]) {
        Ok(out) => {
            let count = out.lines().filter(|l| !l.is_empty()).count() as u32;
            st.dirty_count = count;
            st.dirty = count > 0;
        }
        Err(e) => {
            st.error = Some(e);
        }
    }

    if let Ok(up) = git(
        path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) {
        let up = up.trim().to_string();
        if !up.is_empty() {
            st.upstream = Some(up);
            if let Ok(counts) = git(
                path,
                &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
            ) {
                // format: "ahead\tbehind"
                let parts: Vec<_> = counts.split_whitespace().collect();
                if parts.len() == 2 {
                    st.ahead = parts[0].parse().ok();
                    st.behind = parts[1].parse().ok();
                }
            }
        }
    }

    if let Ok(subj) = git(path, &["log", "-1", "--pretty=%s"]) {
        let s = subj.trim();
        if !s.is_empty() {
            st.head_subject = Some(s.chars().take(72).collect());
        }
    }

    st
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("git {:?} failed", args)
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn print_table(report: &StatusReport, style: &crate::ui::StyleCtx) {
    println!("{}", style.header("repoly status"));
    println!("{}", style.meta_line("workspace", &report.workspace));
    println!("{}", style.meta_line("root", &report.root));
    println!();

    let header = format!(
        "{:<18} {:<22} {:<8} {:>5} {:>6} SUBJECT",
        "ID", "BRANCH", "DIRTY", "AHEAD", "BEHIND"
    );
    println!("{}", style.dim(&header));
    println!("{}", style.rule(90));

    for r in &report.repos {
        if !r.exists {
            let dirty = style.red(&format!("{:<8}", "missing"));
            println!(
                "{} {:<22} {} {:>5} {:>6} {}",
                style.bold(&format!("{:<18}", r.id)),
                "-",
                dirty,
                "-",
                "-",
                style.dim(r.error.as_deref().unwrap_or(""))
            );
            continue;
        }
        if !r.is_git {
            println!(
                "{} {:<22} {:<8} {:>5} {:>6} {}",
                style.bold(&format!("{:<18}", r.id)),
                "-",
                "n/a",
                "-",
                "-",
                style.dim("(not a git repo)")
            );
            continue;
        }
        let dirty = if r.dirty {
            style.yellow(&format!("{:<8}", r.dirty_count))
        } else {
            style.dim(&format!("{:<8}", "clean"))
        };
        let ahead = r.ahead.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        let behind = r
            .behind
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into());
        let branch = r.branch.as_deref().unwrap_or("-");
        let subject = r.head_subject.as_deref().unwrap_or("");
        let err = r
            .error
            .as_ref()
            .map(|e| format!(" {}", style.red(&format!("!{e}"))))
            .unwrap_or_default();
        println!(
            "{} {:<22} {} {:>5} {:>6} {}{}",
            style.bold(&format!("{:<18}", r.id)),
            branch,
            dirty,
            ahead,
            behind,
            style.dim(subject),
            err
        );
    }
}

/// One-line summary for context packs.
pub fn one_liner(s: &RepoStatus) -> String {
    if !s.exists {
        return "missing".into();
    }
    if !s.is_git {
        return "not a git repo".into();
    }
    let branch = s.branch.as_deref().unwrap_or("?");
    let dirty = if s.dirty {
        format!("dirty:{}", s.dirty_count)
    } else {
        "clean".into()
    };
    let ab = match (s.ahead, s.behind) {
        (Some(a), Some(b)) if a > 0 || b > 0 => format!(" ↑{a}↓{b}"),
        _ => String::new(),
    };
    format!("{branch}, {dirty}{ab}")
}
