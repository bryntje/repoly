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

    let mut sections: Vec<Section> = Vec::new();
    let mut truncated = false;
    let mut used = 0usize;

    // Always docs first
    for rel in &workspace.context.always {
        let path = workspace.root.join(rel);
        if !path.is_file() {
            continue;
        }
        if should_skip_file(workspace, rel, &path) {
            continue;
        }
        match read_budgeted(&path, max_chars.saturating_sub(used), &mut truncated) {
            Some(content) => {
                used += content.len();
                sections.push(Section::AlwaysDoc {
                    path: path.display().to_string(),
                    content,
                });
            }
            None => truncated = true,
        }
    }

    // Optional status doc
    if let Some(rel) = &workspace.context.status_doc {
        let path = workspace.root.join(rel);
        if path.is_file() && !should_skip_file(workspace, rel, &path) {
            // Cap status doc more aggressively
            let cap = (max_chars.saturating_sub(used)).min(8_000);
            if let Some(content) = read_budgeted(&path, cap, &mut truncated) {
                used += content.len();
                sections.push(Section::StatusDoc {
                    path: path.display().to_string(),
                    content,
                });
            }
        }
    }

    // If no query and no filters → include all repos as metadata only (no full file dump)
    let full_file_mode = query.map(|q| !q.trim().is_empty()).unwrap_or(false)
        || repos_filter.is_some()
        || tags_filter.is_some()
        || role_filter.is_some();

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
                let per_file = remaining.min(12_000);
                if let Some(content) = read_budgeted(&fp, per_file, &mut truncated) {
                    used += content.len();
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
    };
    let rendered = format_prompt(&pack);
    pack.chars = rendered.chars().count();
    if pack.chars > max_chars {
        pack.truncated = true;
    }
    Ok(pack)
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
    if pack.truncated {
        out.push_str("- **truncated:** yes\n");
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
    out.push_str(&format!("# Root: {}\n\n", pack.root));

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
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ContextSection, RepoEntry};
    use std::path::PathBuf;

    fn sample_ws() -> Workspace {
        Workspace {
            name: "acme".into(),
            root: PathBuf::from("/tmp/acme"),
            config_path: PathBuf::from("/tmp/acme/repoly.toml"),
            context: ContextSection::default(),
            policy: Default::default(),
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
        let tags = vec!["frontend".to_string()];
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
        assert!(ids.contains(&"web"));
        assert!(ids.contains(&"api"));
    }
}
