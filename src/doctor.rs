//! `repoly doctor` — workspace health check for polyrepo + agent hosts.

use crate::config::Workspace;
use crate::context::{self, measure_always_on_disk, measure_status_on_disk};
use crate::discover;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Max untracked sibling suggestions (keep doctor output bounded).
const MAX_UNTRACKED_SUGGESTIONS: usize = 50;

/// Directory names never suggested as missing repos.
const SKIP_DIR_NAMES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    "vendor",
    "coverage",
    ".idea",
    ".vscode",
    ".cursor",
    ".repoly",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warn,
    Error,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub workspace: String,
    pub root: String,
    pub config_path: String,
    pub checks: Vec<Check>,
    pub ok: usize,
    pub warnings: usize,
    pub errors: usize,
}

pub fn run(workspace: &Workspace, config_path: &Path) -> DoctorReport {
    let mut checks = Vec::new();

    // Repos / paths / git
    let mut missing = 0usize;
    let mut not_git = 0usize;
    for repo in &workspace.repos {
        let p = workspace.repo_path(repo);
        if !p.exists() {
            missing += 1;
            checks.push(Check {
                severity: Severity::Error,
                code: "repo_path".into(),
                message: format!("repo '{}' path does not exist: {}", repo.id, p.display()),
            });
            continue;
        }
        if !p.join(".git").exists() {
            // allow bare dirs for meta-ish folders but warn
            not_git += 1;
            checks.push(Check {
                severity: Severity::Warn,
                code: "repo_git".into(),
                message: format!(
                    "repo '{}' is not a git repository: {}",
                    repo.id,
                    p.display()
                ),
            });
        }
    }
    if missing == 0 {
        checks.push(Check {
            severity: Severity::Ok,
            code: "repos".into(),
            message: format!("{} repos; all paths exist", workspace.repos.len()),
        });
    }
    if not_git == 0 && missing == 0 {
        checks.push(Check {
            severity: Severity::Ok,
            code: "git".into(),
            message: "all existing repos look like git checkouts".into(),
        });
    }

    // Soft depends_on warnings
    for w in workspace.validate(false) {
        checks.push(Check {
            severity: Severity::Warn,
            code: "depends_on".into(),
            message: w,
        });
    }

    // Sibling git dirs not listed in repoly.toml (info only — never auto-mutate)
    for c in suggest_untracked_repos(workspace) {
        checks.push(c);
    }

    // Always-docs
    let max_chars = workspace.max_chars();
    let always_disk = measure_always_on_disk(workspace);
    let pct = workspace.repo_reserve_pct() as usize;
    let repo_reserve = max_chars * pct / 100;
    let mut always_cap = max_chars.saturating_sub(repo_reserve);
    if let Some(hard) = workspace.context.always_max_chars {
        always_cap = always_cap.min(hard);
    }

    for doc in &workspace.context.always {
        let rel = doc.path();
        let p = workspace.root.join(rel);
        if !p.is_file() {
            checks.push(Check {
                severity: Severity::Warn,
                code: "always_missing".into(),
                message: format!("always-doc missing: {}", p.display()),
            });
        }
    }

    if always_disk > always_cap {
        checks.push(Check {
            severity: Severity::Warn,
            code: "always_budget".into(),
            message: format!(
                "always-docs on disk ~{always_disk} bytes exceed work-mode always budget {always_cap} \
                 (max_chars={max_chars}, repo_reserve_pct={pct} → reserve ~{repo_reserve}). \
                 ctx/build_context will truncate always and may still skip large status_doc. \
                 Fix: raise max_chars, set always_max_chars, shrink always, or lower repo_reserve_pct"
            ),
        });
    } else if !workspace.context.always.is_empty() {
        checks.push(Check {
            severity: Severity::Ok,
            code: "always_budget".into(),
            message: format!(
                "always-docs ~{always_disk} bytes fit under work-mode always budget {always_cap} \
                 (max_chars={max_chars}, reserve_pct={pct})"
            ),
        });
    }

    if let Some(sz) = measure_status_on_disk(workspace) {
        if sz > 8_000 {
            checks.push(Check {
                severity: Severity::Info,
                code: "status_doc".into(),
                message: format!(
                    "status_doc is ~{sz} bytes; pack only takes up to 8k and only if budget remains after always"
                ),
            });
        }
    }

    // Ranking
    let groups = workspace.ranking.synonym_groups.len();
    if groups > 0 {
        checks.push(Check {
            severity: Severity::Ok,
            code: "ranking".into(),
            message: format!("{groups} synonym group(s) in [ranking]"),
        });
    } else {
        checks.push(Check {
            severity: Severity::Info,
            code: "ranking".into(),
            message: "no [ranking].synonym_groups (built-in expand_token only)".into(),
        });
    }

    // Policy
    if !workspace.policy.skip_globs.is_empty() {
        checks.push(Check {
            severity: Severity::Ok,
            code: "policy".into(),
            message: format!(
                "{} skip_glob(s); audit_log={}",
                workspace.policy.skip_globs.len(),
                workspace.policy.audit_log.as_deref().unwrap_or("(none)")
            ),
        });
    }

    // Tooling
    if Command::new("git").arg("--version").output().is_ok() {
        checks.push(Check {
            severity: Severity::Ok,
            code: "tool_git".into(),
            message: "git is available on PATH".into(),
        });
    } else {
        checks.push(Check {
            severity: Severity::Error,
            code: "tool_git".into(),
            message: "git not found on PATH".into(),
        });
    }

    // MCP tips (informational)
    checks.push(Check {
        severity: Severity::Info,
        code: "mcp".into(),
        message: "MCP: start with `repoly mcp` (read-only). For agents add --allow-exec \
                  --exec-repos a,b and optional --exec-timeout-secs 120. Default bin deny is on."
            .into(),
    });
    checks.push(Check {
        severity: Severity::Info,
        code: "ctx_workflow".into(),
        message: "ctx workflow: plan → build_context with narrow --repos and enough max_chars \
                  (often 90000–120000 when always-docs are large) → edit selected repos only."
            .into(),
    });

    // Smoke: build a pack with the workspace's real max_chars (same budget as `repoly ctx`).
    // Previously capped at 64k, which produced false "always exceed budget" tips when
    // the config used a larger max_chars (e.g. 96k–120k).
    if missing == 0 {
        let first_id = workspace.repos.first().map(|r| r.id.clone());
        if let Some(id) = first_id {
            let ids = vec![id.clone()];
            if let Ok(pack) = context::build_context(
                workspace,
                None,
                Some(&ids),
                None,
                None,
                Some(max_chars),
                true,
                false,
            ) {
                checks.push(Check {
                    severity: Severity::Ok,
                    code: "ctx_smoke".into(),
                    message: format!(
                        "ctx smoke ok · max_chars={max_chars} · always {}/{} · repo_files {} · tips {}",
                        pack.budget.always_bytes,
                        pack.budget.always_cap,
                        pack.budget.repo_files_included,
                        pack.budget.tips.len()
                    ),
                });
                for tip in pack.budget.tips.iter().take(3) {
                    checks.push(Check {
                        severity: Severity::Info,
                        code: "ctx_tip".into(),
                        message: tip.clone(),
                    });
                }
            } else {
                checks.push(Check {
                    severity: Severity::Warn,
                    code: "ctx_smoke".into(),
                    message: format!("ctx smoke failed for repo '{id}' with max_chars={max_chars}"),
                });
            }
        }
    }

    let ok = checks.iter().filter(|c| c.severity == Severity::Ok).count();
    let warnings = checks
        .iter()
        .filter(|c| c.severity == Severity::Warn)
        .count();
    let errors = checks
        .iter()
        .filter(|c| c.severity == Severity::Error)
        .count();

    DoctorReport {
        workspace: workspace.name.clone(),
        root: workspace.root.display().to_string(),
        config_path: config_path.display().to_string(),
        checks,
        ok,
        warnings,
        errors,
    }
}

/// Scan workspace root (depth 1) for git checkouts not declared in config.
fn suggest_untracked_repos(workspace: &Workspace) -> Vec<Check> {
    let mut checks = Vec::new();
    let configured = configured_path_keys(workspace);

    let Ok(entries) = fs::read_dir(&workspace.root) else {
        checks.push(Check {
            severity: Severity::Warn,
            code: "scan_root".into(),
            message: format!(
                "could not read workspace root for untracked-repo scan: {}",
                workspace.root.display()
            ),
        });
        return checks;
    };

    let mut found = 0usize;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        if found >= MAX_UNTRACKED_SUGGESTIONS {
            checks.push(Check {
                severity: Severity::Info,
                code: "untracked_repo_truncated".into(),
                message: format!(
                    "untracked-repo scan stopped after {MAX_UNTRACKED_SUGGESTIONS} suggestions"
                ),
            });
            break;
        }

        let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') || SKIP_DIR_NAMES.contains(&name) {
            continue;
        }
        if !dir.join(".git").exists() {
            continue;
        }

        // Path as it would appear relative to workspace root
        let rel = format!("./{name}");
        let keys = path_match_keys(&dir, name, &rel);
        if keys.iter().any(|k| configured.contains(k)) {
            continue;
        }

        let id = discover::slugify(name);
        let id = if id.is_empty() {
            "repo".to_string()
        } else {
            id
        };
        let (role, tags) = discover::infer_role_and_tags(name, &rel);
        let mut suggest = format!("[[repos]] id = \"{id}\" path = \"{rel}\"");
        if let Some(r) = role {
            suggest.push_str(&format!(" role = \"{r}\""));
        }
        if !tags.is_empty() {
            let t = tags
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ");
            suggest.push_str(&format!(" tags = [{t}]"));
        }

        checks.push(Check {
            severity: Severity::Info,
            code: "untracked_repo".into(),
            message: format!("git dir '{name}' not in repoly.toml — suggest: {suggest}"),
        });
        found += 1;
    }

    if found == 0 {
        // Only emit ok when we successfully scanned and found nothing new
        if workspace.root.is_dir() {
            checks.push(Check {
                severity: Severity::Ok,
                code: "untracked_repos".into(),
                message: "no untracked sibling git dirs under workspace root".into(),
            });
        }
    }

    checks
}

fn configured_path_keys(workspace: &Workspace) -> HashSet<String> {
    let mut set = HashSet::new();
    for repo in &workspace.repos {
        set.insert(normalize_key(&repo.id));
        set.insert(normalize_key(&repo.path));
        // basename of path
        if let Some(base) = Path::new(&repo.path).file_name().and_then(|s| s.to_str()) {
            set.insert(normalize_key(base));
        }
        let abs = workspace.repo_path(repo);
        if let Ok(canon) = abs.canonicalize() {
            set.insert(normalize_key(&canon.to_string_lossy()));
        }
        set.insert(normalize_key(&abs.to_string_lossy()));
    }
    set
}

fn path_match_keys(dir: &Path, name: &str, rel: &str) -> Vec<String> {
    let mut keys = vec![
        normalize_key(name),
        normalize_key(rel),
        normalize_key(&dir.to_string_lossy()),
    ];
    if let Ok(canon) = dir.canonicalize() {
        keys.push(normalize_key(&canon.to_string_lossy()));
    }
    keys
}

fn normalize_key(s: &str) -> String {
    let s = s.trim().trim_start_matches("./").trim_end_matches('/');
    // macOS default FS is case-insensitive; fold for matching
    s.replace('\\', "/").to_lowercase()
}

pub fn format_human(report: &DoctorReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "repoly doctor — workspace `{}`\n  config: {}\n  root:   {}\n\n",
        report.workspace, report.config_path, report.root
    ));
    for c in &report.checks {
        let mark = match c.severity {
            Severity::Ok => "ok  ",
            Severity::Warn => "warn",
            Severity::Error => "err ",
            Severity::Info => "info",
        };
        out.push_str(&format!("  [{mark}] {}\n", c.message));
    }
    out.push_str(&format!(
        "\nsummary: {} ok · {} warn · {} error\n",
        report.ok, report.warnings, report.errors
    ));
    if report.errors > 0 {
        out.push_str("result: FAIL\n");
    } else if report.warnings > 0 {
        out.push_str("result: PASS with warnings\n");
    } else {
        out.push_str("result: PASS\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContextSection, RepoEntry, Workspace};
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_git(dir: &Path) {
        let _ = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status();
    }

    #[test]
    fn suggests_untracked_sibling_git_dir() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        // configured repo
        let api = root.join("api");
        fs::create_dir_all(&api).unwrap();
        init_git(&api);
        // untracked sibling
        let extra = root.join("payments-svc");
        fs::create_dir_all(&extra).unwrap();
        init_git(&extra);
        // junk (no git)
        fs::create_dir_all(root.join("node_modules")).unwrap();

        let ws = Workspace {
            name: "t".into(),
            root: root.to_path_buf(),
            config_path: root.join("repoly.toml"),
            context: ContextSection::default(),
            policy: Default::default(),
            ranking: Default::default(),
            repos: vec![RepoEntry {
                id: "api".into(),
                path: "./api".into(),
                role: Some("api".into()),
                tags: vec![],
                depends_on: vec![],
                description: None,
                context_files: None,
            }],
        };

        let checks = suggest_untracked_repos(&ws);
        let msgs: Vec<_> = checks.iter().map(|c| c.message.as_str()).collect();
        assert!(
            checks.iter().any(|c| c.code == "untracked_repo"
                && c.severity == Severity::Info
                && c.message.contains("payments-svc")),
            "expected untracked_repo for payments-svc, got {msgs:?}"
        );
        assert!(
            !checks.iter().any(|c| c.message.contains("node_modules")),
            "should skip node_modules"
        );
        assert!(
            !checks
                .iter()
                .any(|c| c.code == "untracked_repo" && c.message.contains("'api'")),
            "configured api must not be suggested"
        );
    }
}
