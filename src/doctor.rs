//! `repoly doctor` — workspace health check for polyrepo + agent hosts.

use crate::config::Workspace;
use crate::context::{self, measure_always_on_disk, measure_status_on_disk};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

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

    // Always-docs
    let max_chars = workspace.max_chars();
    let always_disk = measure_always_on_disk(workspace);
    let pct = workspace.repo_reserve_pct() as usize;
    let repo_reserve = max_chars * pct / 100;
    let mut always_cap = max_chars.saturating_sub(repo_reserve);
    if let Some(hard) = workspace.context.always_max_chars {
        always_cap = always_cap.min(hard);
    }

    for rel in &workspace.context.always {
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

    // Smoke: can we build a pack for the first repo?
    if missing == 0 {
        let first_id = workspace.repos.first().map(|r| r.id.clone());
        if let Some(id) = first_id {
            let ids = vec![id];
            if let Ok(pack) = context::build_context(
                workspace,
                None,
                Some(&ids),
                None,
                None,
                Some(max_chars.min(64_000)),
                true,
                false,
            ) {
                checks.push(Check {
                    severity: Severity::Ok,
                    code: "ctx_smoke".into(),
                    message: format!(
                        "ctx smoke ok · always {}/{} · repo_files {} · tips {}",
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
