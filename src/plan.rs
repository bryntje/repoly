//! `repoly plan` — select repos for a task and order them by depends_on.

use crate::config::{RepoEntry, Workspace};
use crate::context;
use crate::status::{self, RepoStatus};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Serialize)]
pub struct WorkPlan {
    pub workspace: String,
    pub root: String,
    pub query: Option<String>,
    /// Tokens actually used for ranking after normalize/rewrite (debug/agents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_normalized: Option<QueryNormalized>,
    pub with_deps: bool,
    pub steps: Vec<PlanStep>,
    pub external_deps: Vec<ExternalDep>,
    pub cycle_warning: Option<String>,
    /// Aggregate live git status across plan steps (omitted when `--no-status`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_summary: Option<PlanStatusSummary>,
    pub suggested: Vec<String>,
}

/// Snapshot of how the free-text query was rewritten before ranking.
#[derive(Debug, Clone, Serialize)]
pub struct QueryNormalized {
    pub original: String,
    pub tokens: Vec<String>,
    pub rewrites_applied: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PlanStatusSummary {
    pub dirty: usize,
    pub behind: usize,
    pub missing: usize,
    pub not_git: usize,
    pub clean: usize,
}

/// Structured git status for one plan step (JSON / agents).
#[derive(Debug, Clone, Serialize)]
pub struct StepStatus {
    pub exists: bool,
    pub is_git: bool,
    pub branch: Option<String>,
    pub dirty: bool,
    pub dirty_count: u32,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStep {
    pub order: usize,
    pub id: String,
    pub path: String,
    pub role: Option<String>,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub depends_on: Vec<String>,
    pub depends_on_in_plan: Vec<String>,
    pub score: i32,
    pub reasons: Vec<String>,
    pub added_as_dependency: bool,
    pub status_line: Option<String>,
    /// Structured status; present unless `--no-status`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<StepStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalDep {
    /// Repo that needs this dependency
    pub from: String,
    /// Dependency id not included in the plan
    pub needs: String,
}

fn step_status_from(s: &RepoStatus) -> StepStatus {
    StepStatus {
        exists: s.exists,
        is_git: s.is_git,
        branch: s.branch.clone(),
        dirty: s.dirty,
        dirty_count: s.dirty_count,
        ahead: s.ahead,
        behind: s.behind,
    }
}

fn summarize_statuses(
    statuses: &HashMap<String, RepoStatus>,
    step_ids: &[String],
) -> PlanStatusSummary {
    let mut sum = PlanStatusSummary::default();
    for id in step_ids {
        let Some(s) = statuses.get(id) else {
            continue;
        };
        if !s.exists {
            sum.missing += 1;
            continue;
        }
        if !s.is_git {
            sum.not_git += 1;
            continue;
        }
        if s.dirty {
            sum.dirty += 1;
        } else {
            sum.clean += 1;
        }
        if s.behind.unwrap_or(0) > 0 {
            sum.behind += 1;
        }
    }
    sum
}

fn format_status_summary_line(sum: &PlanStatusSummary) -> String {
    format!(
        "Status snapshot: {} dirty · {} behind · {} missing · {} not-git · {} clean",
        sum.dirty, sum.behind, sum.missing, sum.not_git, sum.clean
    )
}

pub fn build_plan(
    workspace: &Workspace,
    query: Option<&str>,
    repos_filter: Option<&[String]>,
    tags_filter: Option<&[String]>,
    role_filter: Option<&str>,
    with_deps: bool,
    no_status: bool,
) -> Result<WorkPlan> {
    if query.map(str::trim).filter(|s| !s.is_empty()).is_none()
        && repos_filter.is_none()
        && tags_filter.is_none()
        && role_filter.is_none()
    {
        bail!("repoly plan needs a query and/or --repos / --tags / --role");
    }

    let primary =
        context::select_repos_scored(workspace, query, repos_filter, tags_filter, role_filter);
    if primary.is_empty() {
        bail!("no repos matched; try different query or filters");
    }

    let primary_ids: HashSet<String> = primary.iter().map(|s| s.repo.id.clone()).collect();
    let mut score_map: HashMap<String, i32> = primary
        .iter()
        .map(|s| (s.repo.id.clone(), s.score))
        .collect();
    let mut reason_map: HashMap<String, Vec<String>> = primary
        .iter()
        .map(|s| (s.repo.id.clone(), s.reasons.clone()))
        .collect();
    let mut added_as_dep: HashSet<String> = HashSet::new();

    // Expand transitive depends_on within the workspace graph.
    let mut selected_ids: HashSet<String> = primary_ids.clone();
    if with_deps {
        let mut stack: Vec<String> = primary_ids.iter().cloned().collect();
        while let Some(id) = stack.pop() {
            let Some(repo) = workspace.repos.iter().find(|r| r.id == id) else {
                continue;
            };
            for dep in &repo.depends_on {
                if workspace.repos.iter().any(|r| &r.id == dep) && selected_ids.insert(dep.clone())
                {
                    added_as_dep.insert(dep.clone());
                    score_map.entry(dep.clone()).or_insert(0);
                    reason_map
                        .entry(dep.clone())
                        .or_insert_with(|| vec![format!("depends_on of {id}")]);
                    stack.push(dep.clone());
                }
            }
        }
    }

    let selected: Vec<&RepoEntry> = workspace
        .repos
        .iter()
        .filter(|r| selected_ids.contains(&r.id))
        .collect();

    // External deps: referenced but not in plan (either missing from workspace or excluded)
    let mut external = Vec::new();
    for repo in &selected {
        for dep in &repo.depends_on {
            if !selected_ids.contains(dep) {
                external.push(ExternalDep {
                    from: repo.id.clone(),
                    needs: dep.clone(),
                });
            }
        }
    }

    let (ordered, cycle_warning) = topo_sort(&selected);

    let ids: Vec<String> = ordered.iter().map(|r| r.id.clone()).collect();
    let (status_by_id, status_summary) = if no_status {
        (HashMap::new(), None)
    } else {
        let report = status::collect_status(workspace, Some(&ids), false);
        let map: HashMap<String, RepoStatus> = report
            .repos
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();
        let summary = summarize_statuses(&map, &ids);
        (map, Some(summary))
    };

    let steps: Vec<PlanStep> = ordered
        .iter()
        .enumerate()
        .map(|(i, repo)| {
            let path = workspace.repo_path(repo);
            let in_plan: Vec<String> = repo
                .depends_on
                .iter()
                .filter(|d| selected_ids.contains(*d))
                .cloned()
                .collect();
            let (status_line, status) = match status_by_id.get(&repo.id) {
                Some(s) => (Some(status::one_liner(s)), Some(step_status_from(s))),
                None => (None, None),
            };
            PlanStep {
                order: i + 1,
                id: repo.id.clone(),
                path: path.display().to_string(),
                role: repo.role.clone(),
                tags: repo.tags.clone(),
                description: repo.description.clone(),
                depends_on: repo.depends_on.clone(),
                depends_on_in_plan: in_plan,
                score: *score_map.get(&repo.id).unwrap_or(&0),
                reasons: reason_map
                    .get(&repo.id)
                    .cloned()
                    .unwrap_or_else(|| vec!["dependency".into()]),
                added_as_dependency: added_as_dep.contains(&repo.id),
                status_line,
                status,
            }
        })
        .collect();

    let order_csv = steps
        .iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>()
        .join(",");

    let mut suggested = vec![
        format!(
            "repoly ctx --repos {order_csv} --format grok{}",
            query.map(|q| format!(" {q:?}")).unwrap_or_default()
        ),
        format!("repoly status --repos {order_csv}"),
    ];
    if let Some(sum) = &status_summary {
        if sum.dirty > 0 {
            suggested.push(
                "note: one or more plan repos are dirty — consider stash/commit before multi-repo edits"
                    .into(),
            );
        }
        if sum.missing > 0 {
            suggested.push(
                "note: one or more plan repos are missing on disk — run `repoly doctor`".into(),
            );
        }
    }
    for s in &steps {
        if !s.added_as_dependency || s.score > 0 {
            suggested.push(format!(
                "repoly exec {} -- grok \"work on this step only\"",
                s.id
            ));
        }
    }

    let query_normalized = query.map(|q| {
        let n = crate::rank::normalize_query(workspace, q);
        QueryNormalized {
            original: n.original,
            tokens: n.tokens,
            rewrites_applied: n.rewrites_applied,
        }
    });

    Ok(WorkPlan {
        workspace: workspace.name.clone(),
        root: workspace.root.display().to_string(),
        query: query.map(|s| s.to_string()),
        query_normalized,
        with_deps,
        steps,
        external_deps: external,
        cycle_warning,
        status_summary,
        suggested,
    })
}

/// Kahn topological sort among selected repos (edges = depends_on within set).
/// On cycle: return partial order + remaining in score/id order with a warning.
fn topo_sort<'a>(repos: &[&'a RepoEntry]) -> (Vec<&'a RepoEntry>, Option<String>) {
    let ids: HashSet<&str> = repos.iter().map(|r| r.id.as_str()).collect();
    let by_id: HashMap<&str, &RepoEntry> = repos.iter().map(|r| (r.id.as_str(), *r)).collect();

    let mut indegree: HashMap<&str, usize> = ids.iter().map(|id| (*id, 0usize)).collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for r in repos {
        for dep in &r.depends_on {
            if ids.contains(dep.as_str()) {
                // edge dep -> r (dep must come first)
                adj.entry(dep.as_str()).or_default().push(r.id.as_str());
                *indegree.entry(r.id.as_str()).or_default() += 1;
            }
        }
    }

    let mut q: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    // Stable: sort queue by id
    let mut q_vec: Vec<&str> = q.drain(..).collect();
    q_vec.sort_unstable();
    q.extend(q_vec);

    let mut ordered = Vec::new();
    while let Some(id) = q.pop_front() {
        if let Some(repo) = by_id.get(id) {
            ordered.push(*repo);
        }
        if let Some(nexts) = adj.get(id) {
            let mut unlocked = Vec::new();
            for n in nexts {
                if let Some(d) = indegree.get_mut(n) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        unlocked.push(*n);
                    }
                }
            }
            unlocked.sort_unstable();
            for n in unlocked {
                q.push_back(n);
            }
        }
    }

    if ordered.len() == repos.len() {
        return (ordered, None);
    }

    // Cycle or unresolved: append remaining sorted by id
    let have: HashSet<&str> = ordered.iter().map(|r| r.id.as_str()).collect();
    let mut rest: Vec<&RepoEntry> = repos
        .iter()
        .copied()
        .filter(|r| !have.contains(r.id.as_str()))
        .collect();
    rest.sort_by(|a, b| a.id.cmp(&b.id));
    let warn = format!(
        "depends_on cycle or unresolved among: {}",
        rest.iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    ordered.extend(rest);
    (ordered, Some(warn))
}

pub fn format_markdown(plan: &WorkPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Plan: {}\n\n",
        plan.query.as_deref().unwrap_or("(filters)")
    ));
    out.push_str(&format!("- **workspace:** {}\n", plan.workspace));
    out.push_str(&format!("- **root:** `{}`\n", plan.root));
    out.push_str(&format!(
        "- **with_deps:** {}\n",
        if plan.with_deps { "yes" } else { "no" }
    ));
    out.push_str(&format!("- **steps:** {}\n", plan.steps.len()));
    if let Some(n) = &plan.query_normalized {
        if !n.rewrites_applied.is_empty() || n.tokens.join(" ") != n.original {
            out.push_str(&format!("- **query tokens:** {}\n", n.tokens.join(", ")));
        }
    }
    if let Some(sum) = &plan.status_summary {
        out.push_str(&format!("- **{}**\n", format_status_summary_line(sum)));
        if sum.dirty > 0 {
            out.push_str(
                "- **hint:** dirty repos in plan — stash/commit before multi-repo work if needed\n",
            );
        }
    }
    out.push('\n');

    if let Some(w) = &plan.cycle_warning {
        out.push_str(&format!("> **warning:** {w}\n\n"));
    }

    out.push_str("## Execution order\n\n");
    for s in &plan.steps {
        let role = s.role.as_deref().unwrap_or("-");
        let st = s.status_line.as_deref().unwrap_or("?");
        let dep_mark = if s.added_as_dependency {
            " *(dependency)*"
        } else {
            ""
        };
        out.push_str(&format!(
            "### {}. `{}` ({role}) [{st}]{dep_mark}\n\n",
            s.order, s.id
        ));
        out.push_str(&format!("- path: `{}`\n", s.path));
        if let Some(d) = &s.description {
            out.push_str(&format!("- description: {d}\n"));
        }
        if !s.tags.is_empty() {
            out.push_str(&format!("- tags: {}\n", s.tags.join(", ")));
        }
        if !s.depends_on_in_plan.is_empty() {
            out.push_str(&format!(
                "- depends_on (in plan): {}\n",
                s.depends_on_in_plan.join(", ")
            ));
        }
        if !s.reasons.is_empty() {
            out.push_str(&format!("- why: {}\n", s.reasons.join("; ")));
        }
        if s.score > 0 {
            out.push_str(&format!("- score: {}\n", s.score));
        }
        out.push('\n');
    }

    if !plan.external_deps.is_empty() {
        out.push_str("## External / missing dependencies\n\n");
        for e in &plan.external_deps {
            out.push_str(&format!(
                "- `{}` needs `{}` (not in plan)\n",
                e.from, e.needs
            ));
        }
        out.push('\n');
    }

    out.push_str("## Suggested next commands\n\n");
    out.push_str("```bash\n");
    for cmd in &plan.suggested {
        out.push_str(cmd);
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

pub fn format_prompt(plan: &WorkPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Poly plan: {}\n",
        plan.query.as_deref().unwrap_or("(filters)")
    ));
    out.push_str(&format!(
        "# Workspace: {} @ {}\n",
        plan.workspace, plan.root
    ));
    if let Some(sum) = &plan.status_summary {
        out.push_str(&format!("# {}\n", format_status_summary_line(sum)));
    }
    out.push('\n');
    out.push_str(
        "Work repos in this order (respect depends_on). Change only these repos unless asked.\n\n",
    );
    for s in &plan.steps {
        let role = s.role.as_deref().unwrap_or("-");
        let st = s.status_line.as_deref().unwrap_or("?");
        let dep = if s.added_as_dependency { " [dep]" } else { "" };
        out.push_str(&format!(
            "{}. {} ({role}) [{st}]{dep}\n   path: {}\n",
            s.order, s.id, s.path
        ));
        if !s.depends_on_in_plan.is_empty() {
            out.push_str(&format!("   after: {}\n", s.depends_on_in_plan.join(", ")));
        }
        if !s.reasons.is_empty() {
            out.push_str(&format!("   why: {}\n", s.reasons.join("; ")));
        }
    }
    if let Some(w) = &plan.cycle_warning {
        out.push_str(&format!("\nWarning: {w}\n"));
    }
    if !plan.external_deps.is_empty() {
        out.push_str("\nMissing deps (not in plan):\n");
        for e in &plan.external_deps {
            out.push_str(&format!("- {} → {}\n", e.from, e.needs));
        }
    }
    out.push_str("\n## Suggested\n");
    for cmd in &plan.suggested {
        out.push_str(&format!("- `{cmd}`\n"));
    }
    out
}

/// Grok-oriented plan: workflow instructions + same body as `prompt`.
pub fn format_grok(plan: &WorkPlan) -> String {
    let mut out = String::new();
    out.push_str("# repoly plan for Grok\n");
    out.push_str(&format!(
        "# task: {}\n",
        plan.query.as_deref().unwrap_or("(filters)")
    ));
    out.push_str(&format!(
        "# workspace: {} @ {}\n",
        plan.workspace, plan.root
    ));
    if let Some(n) = &plan.query_normalized {
        out.push_str(&format!("# tokens: {}\n", n.tokens.join(", ")));
        if !n.rewrites_applied.is_empty() {
            out.push_str(&format!("# rewrites: {}\n", n.rewrites_applied.join("; ")));
        }
    }
    if let Some(sum) = &plan.status_summary {
        out.push_str(&format!("# {}\n", format_status_summary_line(sum)));
        if sum.dirty > 0 {
            out.push_str("# hint: dirty working trees in plan — stash/commit if multi-repo edits risk conflict\n");
        }
        if sum.missing > 0 {
            out.push_str("# hint: missing repo paths — run `repoly doctor`\n");
        }
    }
    out.push('\n');

    out.push_str("## Instructions\n");
    out.push_str("- Work repos in **execution order** below (respect depends_on).\n");
    out.push_str("- Change only these repos unless the user expands scope.\n");
    out.push_str(
        "- Next: `repoly ctx --format grok --repos <ordered ids>` (or MCP `build_context`).\n",
    );
    out.push_str("- Commit per product repo; meta/docs are context, not product code.\n\n");

    out.push_str("## Execution order\n\n");
    for s in &plan.steps {
        let role = s.role.as_deref().unwrap_or("-");
        let st = s.status_line.as_deref().unwrap_or("?");
        let dep = if s.added_as_dependency { " [dep]" } else { "" };
        out.push_str(&format!(
            "{}. {} ({role}) [{st}]{dep}\n   path: {}\n",
            s.order, s.id, s.path
        ));
        if !s.depends_on_in_plan.is_empty() {
            out.push_str(&format!("   after: {}\n", s.depends_on_in_plan.join(", ")));
        }
        if !s.reasons.is_empty() {
            out.push_str(&format!("   why: {}\n", s.reasons.join("; ")));
        }
    }
    if let Some(w) = &plan.cycle_warning {
        out.push_str(&format!("\nWarning: {w}\n"));
    }
    if !plan.external_deps.is_empty() {
        out.push_str("\nMissing deps (not in plan):\n");
        for e in &plan.external_deps {
            out.push_str(&format!("- {} → {}\n", e.from, e.needs));
        }
    }
    out.push_str("\n## Next\n");
    for cmd in &plan.suggested {
        out.push_str(&format!("- `{cmd}`\n"));
    }
    out
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
                    id: "core".into(),
                    path: "./core".into(),
                    role: Some("api".into()),
                    tags: vec!["identity".into(), "oauth".into()],
                    depends_on: vec![],
                    description: Some("API".into()),
                    context_files: None,
                },
                RepoEntry {
                    id: "styling".into(),
                    path: "./styling".into(),
                    role: Some("lib".into()),
                    tags: vec!["ui".into()],
                    depends_on: vec![],
                    description: None,
                    context_files: None,
                },
                RepoEntry {
                    id: "app".into(),
                    path: "./app".into(),
                    role: Some("frontend".into()),
                    tags: vec!["identity".into(), "oauth".into()],
                    depends_on: vec!["core".into(), "styling".into()],
                    description: Some("App".into()),
                    context_files: None,
                },
                RepoEntry {
                    id: "mind".into(),
                    path: "./mind".into(),
                    role: Some("frontend".into()),
                    tags: vec!["identity".into()],
                    depends_on: vec!["core".into()],
                    description: None,
                    context_files: None,
                },
            ],
        }
    }

    #[test]
    fn plan_orders_deps_first() {
        let w = ws();
        let plan = build_plan(&w, Some("oauth identity"), None, None, None, true, true).unwrap();
        let ids: Vec<_> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        // core and styling before app
        let i_core = ids.iter().position(|x| *x == "core").unwrap();
        let i_style = ids.iter().position(|x| *x == "styling").unwrap();
        let i_app = ids.iter().position(|x| *x == "app").unwrap();
        assert!(i_core < i_app);
        assert!(i_style < i_app);
        assert!(plan
            .steps
            .iter()
            .any(|s| s.id == "styling" && s.added_as_dependency));
    }

    #[test]
    fn plan_without_deps_skips_styling() {
        let w = ws();
        let plan = build_plan(&w, Some("oauth"), None, None, None, false, true).unwrap();
        assert!(!plan.steps.iter().any(|s| s.id == "styling"));
        assert!(plan
            .external_deps
            .iter()
            .any(|e| e.from == "app" && e.needs == "styling"));
    }

    #[test]
    fn topo_cycle_warns() {
        let a = RepoEntry {
            id: "a".into(),
            path: "./a".into(),
            role: None,
            tags: vec![],
            depends_on: vec!["b".into()],
            description: None,
            context_files: None,
        };
        let b = RepoEntry {
            id: "b".into(),
            path: "./b".into(),
            role: None,
            tags: vec![],
            depends_on: vec!["a".into()],
            description: None,
            context_files: None,
        };
        let repos = vec![&a, &b];
        let (_ord, warn) = topo_sort(&repos);
        assert!(warn.is_some());
    }
}
