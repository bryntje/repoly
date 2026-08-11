use crate::config::{RepoEntry, Workspace, DEFAULT_CONTEXT_FILES};
use crate::policy;
use crate::rank;
use crate::status::{self, RepoStatus};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ContextPack {
    pub workspace: String,
    pub root: String,
    pub query: Option<String>,
    pub selected_repos: Vec<String>,
    pub sections: Vec<Section>,
    pub truncated: bool,
    pub chars: usize,
    /// How the pack budget was spent (for agents and `repoly doctor`).
    pub budget: PackBudget,
}

/// Budget accounting for a context pack.
#[derive(Debug, Clone, Serialize, Default)]
pub struct PackBudget {
    pub max_chars: usize,
    /// Cap applied to always-docs (may be less than max_chars when repo reserve is on).
    pub always_cap: usize,
    /// Bytes reserved for selected-repo context files (work mode only).
    pub repo_reserve: usize,
    pub always_bytes: usize,
    pub status_bytes: usize,
    pub repo_file_bytes: usize,
    pub repo_files_included: usize,
    /// Human tips when the pack is thin or truncated.
    pub tips: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Section {
    #[serde(rename = "always_doc")]
    AlwaysDoc { path: String, content: String },
    #[serde(rename = "status_doc")]
    StatusDoc { path: String, content: String },
    #[serde(rename = "repo")]
    Repo {
        id: String,
        path: String,
        role: Option<String>,
        tags: Vec<String>,
        description: Option<String>,
        depends_on: Vec<String>,
        status_line: Option<String>,
        files: Vec<FileChunk>,
    },
    #[serde(rename = "repo_meta")]
    RepoMeta {
        id: String,
        path: String,
        role: Option<String>,
        tags: Vec<String>,
        description: Option<String>,
        depends_on: Vec<String>,
        status_line: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct FileChunk {
    pub path: String,
    pub content: String,
}

#[allow(clippy::too_many_arguments)]
pub fn build_context(
    workspace: &Workspace,
    query: Option<&str>,
    repos_filter: Option<&[String]>,
    tags_filter: Option<&[String]>,
    role_filter: Option<&str>,
    max_chars_override: Option<usize>,
    no_status: bool,
    with_deps: bool,
) -> Result<ContextPack> {
    let max_chars = max_chars_override.unwrap_or_else(|| workspace.max_chars());
    let mut selected = select_repos(workspace, query, repos_filter, tags_filter, role_filter);
    if with_deps {
        selected = expand_with_deps(workspace, &selected);
    }
    let selected_ids: Vec<String> = selected.iter().map(|r| r.id.clone()).collect();

    // Live status for selected (and for full meta listing we still may want status)
    let status_map: std::collections::HashMap<String, RepoStatus> = if no_status {
        Default::default()
    } else {
        let report = status::collect_status(workspace, Some(&selected_ids), false);
        report
            .repos
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect()
    };

    // If no query and no filters → include all repos as metadata only (no full file dump)
    let full_file_mode = query.map(|q| !q.trim().is_empty()).unwrap_or(false)
        || repos_filter.is_some()
        || tags_filter.is_some()
        || role_filter.is_some();

    let (always_cap, repo_reserve) = work_mode_budgets(workspace, max_chars, full_file_mode);

    let mut sections: Vec<Section> = Vec::new();
    let mut truncated = false;
    let mut used = 0usize;
    let mut always_bytes = 0usize;
    let mut status_bytes = 0usize;
    let mut repo_file_bytes = 0usize;
    let mut repo_files_included = 0usize;

    // Always docs first — limited by always_cap so work mode keeps room for repos
    let mut always_used = 0usize;
    for doc in &workspace.context.always {
        let rel = doc.path();
        let path = workspace.root.join(rel);
        if !path.is_file() {
            continue;
        }
        if should_skip_file(workspace, rel, &path) {
            continue;
        }

        // Tag-based conditional always-doc (C)
        let doc_tags = doc.tags();
        if !doc_tags.is_empty() {
            let selected_tags: std::collections::HashSet<&str> = selected
                .iter()
                .flat_map(|r| r.tags.iter().map(|s| s.as_str()))
                .collect();
            if !doc_tags.iter().any(|t| selected_tags.contains(t.as_str())) {
                continue;
            }
        }
        let room = always_cap.saturating_sub(always_used);
        if room == 0 {
            truncated = true;
            break;
        }
        // Use smart section extraction when possible
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let mut secs = crate::always::extract_sections(&raw);
            crate::always::score_sections(
                &mut secs,
                query,
                &selected
                    .iter()
                    .flat_map(|r| r.tags.iter().cloned())
                    .collect::<Vec<_>>(),
                doc.tags(),
                doc.sections(),
            );
            let (chosen, sec_trunc) = crate::always::select_relevant_sections(secs, room);
            if sec_trunc {
                truncated = true;
            }

            for s in chosen {
                let n = s.content.len();
                if always_used + n > room {
                    break;
                }
                always_used += n;
                always_bytes += n;
                used += n;

                let header = if s.title == "Top" {
                    String::new()
                } else {
                    format!("## {}\n", s.title)
                };
                sections.push(Section::AlwaysDoc {
                    path: format!("{}#{}", path.display(), s.title),
                    content: format!("{}{}", header, s.content),
                });
            }
        } else {
            // Fallback to full file
            if let Some(content) = read_budgeted(&path, room, &mut truncated) {
                let n = content.len();
                always_used += n;
                always_bytes += n;
                used += n;
                sections.push(Section::AlwaysDoc {
                    path: path.display().to_string(),
                    content,
                });
            }
        }
    }

    // Optional status doc — never eat the repo reserve in work mode
    if let Some(rel) = &workspace.context.status_doc {
        let path = workspace.root.join(rel);
        if path.is_file() && !should_skip_file(workspace, rel, &path) {
            let leave_for_repos = if full_file_mode { repo_reserve } else { 0 };
            let room = max_chars
                .saturating_sub(used)
                .saturating_sub(leave_for_repos);
            let cap = room.min(8_000);
            if cap > 0 {
                if let Some(content) = read_budgeted(&path, cap, &mut truncated) {
                    status_bytes += content.len();
                    used += content.len();
                    sections.push(Section::StatusDoc {
                        path: path.display().to_string(),
                        content,
                    });
                }
            } else {
                truncated = true;
            }
        }
    }

    if full_file_mode {
        for repo in &selected {
            if used >= max_chars {
                truncated = true;
                break;
            }
            let path = workspace.repo_path(repo);
            let status_line = status_map.get(&repo.id).map(status::one_liner);
            let mut files = Vec::new();
            let context_files = repo
                .context_files
                .as_ref()
                .map(|v| v.iter().map(|s| s.as_str()).collect::<Vec<_>>())
                .unwrap_or_else(|| DEFAULT_CONTEXT_FILES.to_vec());

            for cf in context_files {
                let fp = path.join(cf);
                if !fp.is_file() || should_skip_file(workspace, cf, &fp) {
                    continue;
                }
                let remaining = max_chars.saturating_sub(used);
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                let per_file = remaining.min(12_000);
                if let Some(content) = read_budgeted(&fp, per_file, &mut truncated) {
                    let n = content.len();
                    used += n;
                    repo_file_bytes += n;
                    repo_files_included += 1;
                    files.push(FileChunk {
                        path: cf.to_string(),
                        content,
                    });
                }
                if !files.is_empty() && used > max_chars * 3 / 4 {
                    break;
                }
            }

            used += estimate_repo_header(repo);
            sections.push(Section::Repo {
                id: repo.id.clone(),
                path: path.display().to_string(),
                role: repo.role.clone(),
                tags: repo.tags.clone(),
                description: repo.description.clone(),
                depends_on: repo.depends_on.clone(),
                status_line,
                files,
            });
        }
    } else {
        // Overview mode: status for every repo once, metadata only
        let all_status = if no_status {
            Default::default()
        } else {
            let report = status::collect_status(workspace, None, false);
            report
                .repos
                .into_iter()
                .map(|s| (s.id.clone(), s))
                .collect::<std::collections::HashMap<_, _>>()
        };

        for repo in &workspace.repos {
            let path = workspace.repo_path(repo);
            sections.push(Section::RepoMeta {
                id: repo.id.clone(),
                path: path.display().to_string(),
                role: repo.role.clone(),
                tags: repo.tags.clone(),
                description: repo.description.clone(),
                depends_on: repo.depends_on.clone(),
                status_line: all_status.get(&repo.id).map(status::one_liner),
            });
        }
    }

    let always_on_disk = measure_always_on_disk(workspace);
    let tips = build_tips(
        full_file_mode,
        max_chars,
        always_cap,
        always_on_disk,
        always_bytes,
        status_bytes,
        repo_files_included,
        truncated,
    );

    let budget = PackBudget {
        max_chars,
        always_cap,
        repo_reserve,
        always_bytes,
        status_bytes,
        repo_file_bytes,
        repo_files_included,
        tips,
    };

    // Recompute chars from rendered prompt for honesty
    let mut pack = ContextPack {
        workspace: workspace.name.clone(),
        root: workspace.root.display().to_string(),
        query: query.map(|s| s.to_string()),
        selected_repos: if full_file_mode {
            selected_ids
        } else {
            workspace.repos.iter().map(|r| r.id.clone()).collect()
        },
        sections,
        truncated,
        chars: 0,
        budget,
    };
    let rendered = format_prompt(&pack);
    pack.chars = rendered.chars().count();
    if pack.chars > max_chars {
        pack.truncated = true;
    }
    Ok(pack)
}

/// Work mode reserves space for repo files; overview mode may use full budget for always.
fn work_mode_budgets(
    workspace: &Workspace,
    max_chars: usize,
    full_file_mode: bool,
) -> (usize, usize) {
    if !full_file_mode {
        let mut always_cap = max_chars;
        if let Some(hard) = workspace.context.always_max_chars {
            always_cap = always_cap.min(hard);
        }
        return (always_cap, 0);
    }
    let pct = workspace.repo_reserve_pct() as usize;
    let repo_reserve = (max_chars * pct / 100).max(1);
    let mut always_cap = max_chars.saturating_sub(repo_reserve);
    if let Some(hard) = workspace.context.always_max_chars {
        always_cap = always_cap.min(hard);
    }
    (always_cap, repo_reserve)
}

/// Sum of on-disk sizes of always-docs (for doctor / tips).
pub fn measure_always_on_disk(workspace: &Workspace) -> usize {
    let mut total = 0usize;
    for doc in &workspace.context.always {
        let rel = doc.path();
        let path = workspace.root.join(rel);
        if path.is_file() {
            if let Ok(meta) = fs::metadata(&path) {
                total += meta.len() as usize;
            }
        }
    }
    total
}

pub fn measure_status_on_disk(workspace: &Workspace) -> Option<usize> {
    let rel = workspace.context.status_doc.as_ref()?;
    let path = workspace.root.join(rel);
    if !path.is_file() {
        return None;
    }
    fs::metadata(&path).ok().map(|m| m.len() as usize)
}

#[allow(clippy::too_many_arguments)]
fn build_tips(
    full_file_mode: bool,
    max_chars: usize,
    always_cap: usize,
    always_on_disk: usize,
    always_bytes: usize,
    status_bytes: usize,
    repo_files_included: usize,
    truncated: bool,
) -> Vec<String> {
    let mut tips = Vec::new();
    if always_on_disk > always_cap {
        tips.push(format!(
            "always-docs on disk (~{always_on_disk} bytes) exceed always budget ({always_cap}); \
             raise max_chars, set always_max_chars, shrink always list, or lower repo_reserve_pct"
        ));
    }
    if full_file_mode && repo_files_included == 0 {
        tips.push(
            "no per-repo context files (AGENTS.md/README/…) fit in the pack; \
             raise --max-chars / context.max_chars or reduce always-docs"
                .into(),
        );
    }
    if full_file_mode && status_bytes == 0 && always_bytes + 1 >= always_cap {
        tips.push(
            "status_doc skipped (budget tight after always); use `repoly status` or raise max_chars"
                .into(),
        );
    }
    if truncated {
        tips.push(
            "pack truncated; narrow --repos / query or raise max_chars for fuller context".into(),
        );
    }
    if max_chars < 24_000 && full_file_mode {
        tips.push(
            "max_chars is low for multi-doc workspaces; 90000–120000 is common for agent packs"
                .into(),
        );
    }
    tips
}

fn estimate_repo_header(repo: &RepoEntry) -> usize {
    80 + repo.id.len() + repo.description.as_ref().map(|s| s.len()).unwrap_or(0)
}

/// Select repos by explicit filters and/or keyword query (same rules as `repoly ctx` / `repoly plan`).
pub fn select_repos<'a>(
    workspace: &'a Workspace,
    query: Option<&str>,
    repos_filter: Option<&[String]>,
    tags_filter: Option<&[String]>,
    role_filter: Option<&str>,
) -> Vec<&'a RepoEntry> {
    select_repos_scored(workspace, query, repos_filter, tags_filter, role_filter)
        .into_iter()
        .map(|s| s.repo)
        .collect()
}

/// Scored selection used by `repoly plan` (includes match score + reason hints).
#[derive(Debug, Clone)]
pub struct ScoredRepo<'a> {
    pub repo: &'a RepoEntry,
    pub score: i32,
    pub reasons: Vec<String>,
}

pub fn select_repos_scored<'a>(
    workspace: &'a Workspace,
    query: Option<&str>,
    repos_filter: Option<&[String]>,
    tags_filter: Option<&[String]>,
    role_filter: Option<&str>,
) -> Vec<ScoredRepo<'a>> {
    let mut set: Vec<&RepoEntry> = workspace.repos.iter().collect();

    if let Some(ids) = repos_filter {
        set.retain(|r| ids.iter().any(|id| id == &r.id));
    }
    if let Some(tags) = tags_filter {
        set.retain(|r| r.tags.iter().any(|t| tags.iter().any(|ft| ft == t)));
    }
    if let Some(role) = role_filter {
        set.retain(|r| r.role.as_deref() == Some(role));
    }

    // Explicit filters only → equal score, reason = filter
    if repos_filter.is_some() || tags_filter.is_some() || role_filter.is_some() {
        // If query also present, re-rank filtered set with smart ranker
        if let Some(q) = query.map(str::trim).filter(|s| !s.is_empty()) {
            let ranked = rank::rank_repos(workspace, q);
            let allowed: HashSet<&str> = set.iter().map(|r| r.id.as_str()).collect();
            let mut out: Vec<ScoredRepo<'_>> = ranked
                .into_iter()
                .filter(|r| allowed.contains(r.repo.id.as_str()))
                .map(|r| ScoredRepo {
                    repo: r.repo,
                    score: r.score,
                    reasons: r.reasons,
                })
                .collect();
            // Keep filter-only repos that didn't score
            for repo in set {
                if !out.iter().any(|s| s.repo.id == repo.id) {
                    out.push(ScoredRepo {
                        repo,
                        score: 1,
                        reasons: vec!["filter".into()],
                    });
                }
            }
            return out;
        }

        return set
            .into_iter()
            .map(|repo| {
                let mut reasons = Vec::new();
                if let Some(ids) = repos_filter {
                    if ids.iter().any(|id| id == &repo.id) {
                        reasons.push("matched --repos".into());
                    }
                }
                if let Some(tags) = tags_filter {
                    let hit: Vec<_> = repo
                        .tags
                        .iter()
                        .filter(|t| tags.iter().any(|ft| ft == *t))
                        .cloned()
                        .collect();
                    if !hit.is_empty() {
                        reasons.push(format!("tags: {}", hit.join(", ")));
                    }
                }
                if let Some(role) = role_filter {
                    if repo.role.as_deref() == Some(role) {
                        reasons.push(format!("role: {role}"));
                    }
                }
                if reasons.is_empty() {
                    reasons.push("filter".into());
                }
                ScoredRepo {
                    repo,
                    score: 1,
                    reasons,
                }
            })
            .collect();
    }

    if let Some(q) = query {
        let q = q.trim();
        if q.is_empty() {
            return set
                .into_iter()
                .map(|repo| ScoredRepo {
                    repo,
                    score: 0,
                    reasons: vec!["all repos (empty query)".into()],
                })
                .collect();
        }
        return rank::rank_repos(workspace, q)
            .into_iter()
            .map(|r| ScoredRepo {
                repo: r.repo,
                score: r.score,
                reasons: r.reasons,
            })
            .collect();
    }

    set.into_iter()
        .map(|repo| ScoredRepo {
            repo,
            score: 0,
            reasons: vec!["all repos".into()],
        })
        .collect()
}

/// Add transitive depends_on of selected repos (workspace-local only).
pub fn expand_with_deps<'a>(
    workspace: &'a Workspace,
    selected: &[&'a RepoEntry],
) -> Vec<&'a RepoEntry> {
    let mut ids: HashSet<String> = selected.iter().map(|r| r.id.clone()).collect();
    let mut stack: Vec<String> = ids.iter().cloned().collect();
    while let Some(id) = stack.pop() {
        if let Some(repo) = workspace.repos.iter().find(|r| r.id == id) {
            for dep in &repo.depends_on {
                if workspace.repos.iter().any(|r| &r.id == dep) && ids.insert(dep.clone()) {
                    stack.push(dep.clone());
                }
            }
        }
    }
    workspace
        .repos
        .iter()
        .filter(|r| ids.contains(&r.id))
        .collect()
}

fn should_skip_file(workspace: &Workspace, relative: &str, path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    policy::should_skip_context_path(
        relative,
        name,
        &workspace.policy.skip_globs,
        workspace.policy.use_builtin_secret_filters,
    )
}

fn read_budgeted(path: &Path, budget: usize, truncated: &mut bool) -> Option<String> {
    if budget == 0 {
        *truncated = true;
        return None;
    }
    let raw = fs::read_to_string(path).ok()?;
    if raw.chars().count() <= budget {
        return Some(raw);
    }
    *truncated = true;
    let mut out: String = raw.chars().take(budget.saturating_sub(48)).collect();
    out.push_str("\n\n… [truncated by repoly; raise --max-chars]\n");
    Some(out)
}

fn budget_header(pack: &ContextPack) -> String {
    let b = &pack.budget;
    let mut s = format!(
        "# budget: max={} always={}/{} status={} repo_files={} ({} bytes) reserve={}\n",
        b.max_chars,
        b.always_bytes,
        b.always_cap,
        b.status_bytes,
        b.repo_files_included,
        b.repo_file_bytes,
        b.repo_reserve,
    );
    if pack.truncated {
        s.push_str("# truncated: yes\n");
    }
    for tip in &b.tips {
        s.push_str(&format!("# tip: {tip}\n"));
    }
    s.push('\n');
    s
}

pub fn format_markdown(pack: &ContextPack) -> String {
    let mut out = String::new();
    out.push_str(&format!("# repoly workspace: {}\n\n", pack.workspace));
    out.push_str(&format!("- **root:** `{}`\n", pack.root));
    if let Some(q) = &pack.query {
        out.push_str(&format!("- **query:** {q}\n"));
    }
    out.push_str(&format!(
        "- **selected:** {}\n",
        pack.selected_repos.join(", ")
    ));
    out.push_str(&format!(
        "- **budget:** max={} · always {}/{} · status {} · repo_files {} ({} B) · reserve {}\n",
        pack.budget.max_chars,
        pack.budget.always_bytes,
        pack.budget.always_cap,
        pack.budget.status_bytes,
        pack.budget.repo_files_included,
        pack.budget.repo_file_bytes,
        pack.budget.repo_reserve,
    ));
    if pack.truncated {
        out.push_str("- **truncated:** yes\n");
    }
    for tip in &pack.budget.tips {
        out.push_str(&format!("- **tip:** {tip}\n"));
    }
    out.push('\n');

    for section in &pack.sections {
        match section {
            Section::AlwaysDoc { path, content } => {
                out.push_str(&format!("## Cross-repo: `{}`\n\n", path));
                out.push_str(content);
                out.push_str("\n\n");
            }
            Section::StatusDoc { path, content } => {
                out.push_str(&format!("## Status doc: `{}`\n\n", path));
                out.push_str(content);
                out.push_str("\n\n");
            }
            Section::Repo {
                id,
                path,
                role,
                tags,
                description,
                depends_on,
                status_line,
                files,
            } => {
                let role_s = role.as_deref().unwrap_or("-");
                let st = status_line.as_deref().unwrap_or("?");
                out.push_str(&format!("## Repo: `{id}` ({role_s}) [{st}]\n\n"));
                out.push_str(&format!("- path: `{path}`\n"));
                if let Some(d) = description {
                    out.push_str(&format!("- description: {d}\n"));
                }
                if !tags.is_empty() {
                    out.push_str(&format!("- tags: {}\n", tags.join(", ")));
                }
                if !depends_on.is_empty() {
                    out.push_str(&format!("- depends_on: {}\n", depends_on.join(", ")));
                }
                out.push('\n');
                for f in files {
                    out.push_str(&format!("### {}\n\n", f.path));
                    out.push_str(&f.content);
                    out.push_str("\n\n");
                }
            }
            Section::RepoMeta {
                id,
                path,
                role,
                tags,
                description,
                depends_on,
                status_line,
            } => {
                let role_s = role.as_deref().unwrap_or("-");
                let st = status_line.as_deref().unwrap_or("?");
                out.push_str(&format!("## `{id}` ({role_s}) — {st}\n"));
                out.push_str(&format!("- path: `{path}`\n"));
                if let Some(d) = description {
                    out.push_str(&format!("- {d}\n"));
                }
                if !tags.is_empty() {
                    out.push_str(&format!("- tags: {}\n", tags.join(", ")));
                }
                if !depends_on.is_empty() {
                    out.push_str(&format!("- depends_on: {}\n", depends_on.join(", ")));
                }
                out.push('\n');
            }
        }
    }
    out
}

pub fn format_prompt(pack: &ContextPack) -> String {
    let mut out = String::new();
    out.push_str(&format!("# repoly workspace: {}\n", pack.workspace));
    if let Some(q) = &pack.query {
        out.push_str(&format!("# Query: {q}\n"));
    }
    out.push_str(&format!("# Selected: {}\n", pack.selected_repos.join(", ")));
    out.push_str(&format!("# Root: {}\n", pack.root));
    out.push_str(&budget_header(pack));

    for section in &pack.sections {
        match section {
            Section::AlwaysDoc { path, content } => {
                out.push_str(&format!("## Cross-repo (always): {path}\n\n"));
                out.push_str(content);
                out.push_str("\n\n");
            }
            Section::StatusDoc { path, content } => {
                out.push_str(&format!("## Living status: {path}\n\n"));
                out.push_str(content);
                out.push_str("\n\n");
            }
            Section::Repo {
                id,
                path,
                role,
                tags,
                description,
                depends_on,
                status_line,
                files,
            } => {
                let role_s = role.as_deref().unwrap_or("-");
                let st = status_line.as_deref().unwrap_or("?");
                out.push_str(&format!("## Repo: {id} ({role_s}) [{st}]\n"));
                out.push_str(&format!("path: {path}\n"));
                if let Some(d) = description {
                    out.push_str(&format!("description: {d}\n"));
                }
                if !tags.is_empty() {
                    out.push_str(&format!("tags: {}\n", tags.join(", ")));
                }
                if !depends_on.is_empty() {
                    out.push_str(&format!("depends_on: {}\n", depends_on.join(", ")));
                }
                out.push('\n');
                for f in files {
                    out.push_str(&format!("### {}\n\n", f.path));
                    out.push_str(&f.content);
                    out.push_str("\n\n");
                }
            }
            Section::RepoMeta {
                id,
                path,
                role,
                tags,
                description,
                depends_on,
                status_line,
            } => {
                let role_s = role.as_deref().unwrap_or("-");
                let st = status_line.as_deref().unwrap_or("?");
                out.push_str(&format!("## {id} ({role_s}) [{st}]\n"));
                out.push_str(&format!("path: {path}\n"));
                if let Some(d) = description {
                    out.push_str(&format!("description: {d}\n"));
                }
                if !tags.is_empty() {
                    out.push_str(&format!("tags: {}\n", tags.join(", ")));
                }
                if !depends_on.is_empty() {
                    out.push_str(&format!("depends_on: {}\n", depends_on.join(", ")));
                }
                out.push('\n');
            }
        }
    }

    out.push_str("## How to work\n");
    out.push_str("- Prefer changes only in selected repos unless the user asks otherwise.\n");
    out.push_str(
        "- Commit in the correct product repo; meta/docs are context, not product code.\n",
    );
    out.push_str("- Do not dump unrelated repos into your working set.\n");
    if pack.truncated {
        out.push_str(
            "- Note: this pack was truncated; ask for a narrower query or higher --max-chars.\n",
        );
    }
    if pack.budget.repo_files_included == 0 && !pack.selected_repos.is_empty() {
        out.push_str(
            "- Note: no per-repo AGENTS/README fit; open those files in the target repos directly.\n",
        );
    }
    out.push('\n');
    out
}

/// Grok-oriented context pack: same body as `prompt`, with explicit agent workflow.
pub fn format_grok(pack: &ContextPack) -> String {
    let mut out = String::new();
    out.push_str("# repoly context for Grok\n");
    out.push_str(&format!(
        "# workspace: {} @ {}\n",
        pack.workspace, pack.root
    ));
    if let Some(q) = &pack.query {
        out.push_str(&format!("# query: {q}\n"));
    }
    out.push_str(&format!("# selected: {}\n", pack.selected_repos.join(", ")));
    out.push_str(&budget_header(pack));

    out.push_str("## Instructions\n");
    out.push_str("- Edit only **selected** repos unless the user expands scope.\n");
    out.push_str(
        "- Workflow: `repoly plan --format grok \"…\"` → this pack → edit → commit per repo.\n",
    );
    out.push_str("- MCP twin: `plan(format=grok)` then `build_context(format=grok, repos=…)`.\n");
    out.push_str("- Paths below are absolute; use them as cwd anchors.\n");
    out.push_str(
        "- Commit in the product repo that owns the change; meta/docs are context only.\n\n",
    );

    // Reuse section body from format_prompt (skip its header + generic "How to work").
    let body = format_prompt(pack);
    let mut sections = if let Some(idx) = body.find("## ") {
        body[idx..].to_string()
    } else {
        body
    };
    if let Some(hw) = sections.find("## How to work") {
        sections.truncate(hw);
    }
    out.push_str(&sections);

    out.push_str("## Next\n");
    if !pack.selected_repos.is_empty() {
        let csv = pack.selected_repos.join(",");
        out.push_str(&format!("- `repoly status --repos {csv}`\n"));
        for id in &pack.selected_repos {
            out.push_str(&format!("- work in `{id}` only unless asked otherwise\n"));
        }
    }
    if pack.truncated {
        out.push_str("- Pack was truncated — narrow query or raise `--max-chars`.\n");
    }
    if pack.budget.repo_files_included == 0 && !pack.selected_repos.is_empty() {
        out.push_str(
            "- No per-repo AGENTS/README fit in budget; open those files in the target repos.\n",
        );
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContextSection, RepoEntry};
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn sample_ws() -> Workspace {
        Workspace {
            name: "acme".into(),
            root: PathBuf::from("/tmp/acme"),
            config_path: PathBuf::from("/tmp/acme/repoly.toml"),
            context: ContextSection::default(),
            policy: Default::default(),
            ranking: Default::default(),
            repos: vec![
                RepoEntry {
                    id: "api".into(),
                    path: "./api".into(),
                    role: Some("api".into()),
                    tags: vec!["backend".into(), "payments".into()],
                    depends_on: vec![],
                    description: Some("Core API".into()),
                    context_files: None,
                },
                RepoEntry {
                    id: "web".into(),
                    path: "./web".into(),
                    role: Some("frontend".into()),
                    tags: vec!["frontend".into()],
                    depends_on: vec!["api".into()],
                    description: Some("Web app".into()),
                    context_files: None,
                },
            ],
        }
    }

    #[test]
    fn query_selects_payments() {
        let ws = sample_ws();
        let sel = select_repos(&ws, Some("payments"), None, None, None);
        assert!(!sel.is_empty());
        assert_eq!(sel[0].id, "api");
    }

    #[test]
    fn tag_filter() {
        let ws = sample_ws();
        let tags = vec!["frontend".into()];
        let sel = select_repos(&ws, None, None, Some(&tags), None);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].id, "web");
    }

    #[test]
    fn expand_deps_includes_api() {
        let ws = sample_ws();
        let web = ws.repos.iter().find(|r| r.id == "web").unwrap();
        let expanded = expand_with_deps(&ws, &[web]);
        let ids: Vec<_> = expanded.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"api"));
        assert!(ids.contains(&"web"));
    }

    #[test]
    fn work_mode_reserves_room_for_repo_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("api")).unwrap();
        // Large always doc
        let mut always = fs::File::create(root.join("ALWAYS.md")).unwrap();
        write!(always, "{}", "A".repeat(20_000)).unwrap();
        // Repo AGENTS
        fs::write(root.join("api/AGENTS.md"), "repo-specific agents rules\n").unwrap();

        let ws = Workspace {
            name: "t".into(),
            root: root.to_path_buf(),
            config_path: root.join("repoly.toml"),
            context: ContextSection {
                always: vec!["ALWAYS.md".into()],
                max_chars: Some(10_000),
                repo_reserve_pct: Some(40),
                ..Default::default()
            },
            policy: Default::default(),
            ranking: Default::default(),
            repos: vec![RepoEntry {
                id: "api".into(),
                path: "api".into(),
                role: Some("api".into()),
                tags: vec![],
                depends_on: vec![],
                description: None,
                context_files: None,
            }],
        };

        let pack = build_context(
            &ws,
            Some("api"),
            Some(&["api".into()]),
            None,
            None,
            None,
            true,
            false,
        )
        .unwrap();

        assert!(
            pack.budget.repo_files_included >= 1,
            "expected repo file with reserve; budget={:?}",
            pack.budget
        );
        assert!(
            pack.budget.always_bytes <= pack.budget.always_cap,
            "always exceeded cap: {:?}",
            pack.budget
        );
        assert!(pack.budget.always_cap < pack.budget.max_chars);
    }
}
