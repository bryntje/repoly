//! MCP (Model Context Protocol) stdio server for poly.
//!
//! Read-only tools so agents can discover multi-repo context without shell hacks.
//! Mutation (`exec` / `run`) stays on the CLI surface on purpose.

use crate::config::{find_config, load_config, Workspace};
use crate::context;
use crate::plan;
use crate::status;
use anyhow::{Context as _, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// Run the MCP server on stdio until the client disconnects.
pub async fn serve(config: Option<PathBuf>) -> Result<()> {
    let server = PolyMcp::new(config)?;
    let service = server.serve(stdio()).await.context("starting MCP stdio transport")?;
    service.waiting().await.context("MCP server session")?;
    Ok(())
}

#[derive(Clone)]
pub struct PolyMcp {
    // Used by #[tool_handler] / ToolRouter macros.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    /// Resolved config path (if known at startup).
    config_path: Option<PathBuf>,
}

impl PolyMcp {
    pub fn new(config: Option<PathBuf>) -> Result<Self> {
        // Eagerly validate config if discoverable so startup fails fast with a clear error.
        if let Some(ref p) = config {
            let _ = load_config(p).with_context(|| format!("loading {}", p.display()))?;
        } else if let Ok(p) = find_config() {
            let _ = load_config(&p).with_context(|| format!("loading {}", p.display()))?;
        }
        // If no config yet, tools will error with a clear message when called
        // (agent may start MCP from a different cwd than the workspace).

        Ok(Self {
            tool_router: Self::tool_router(),
            config_path: config.or_else(|| find_config().ok()),
        })
    }

    fn load_ws(&self) -> Result<Workspace, McpError> {
        let path = if let Some(ref p) = self.config_path {
            p.clone()
        } else {
            find_config().map_err(|e| {
                McpError::invalid_params(
                    format!(
                        "workspace not found ({e}); start poly mcp from a directory with poly.toml \
                         or set POLY_CONFIG / pass --config"
                    ),
                    None,
                )
            })?
        };
        load_config(&path).map_err(|e| {
            McpError::invalid_params(format!("failed to load {}: {e}", path.display()), None)
        })
    }
}

// ── Tool parameter types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
struct EmptyArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
struct StatusArgs {
    /// Comma-separated repo ids to include (optional; default all).
    #[serde(default)]
    repos: Option<String>,
    /// Run `git fetch` before computing ahead/behind (default false).
    #[serde(default)]
    fetch: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CtxArgs {
    /// Free-text keyword query for repo selection.
    #[serde(default)]
    query: Option<String>,
    /// Comma-separated repo ids to force-include.
    #[serde(default)]
    repos: Option<String>,
    /// Comma-separated tags; match any.
    #[serde(default)]
    tags: Option<String>,
    /// Filter by role (e.g. "api", "frontend").
    #[serde(default)]
    role: Option<String>,
    /// Output format: "prompt" (default), "markdown", or "json".
    #[serde(default)]
    format: Option<String>,
    /// Max characters for the pack.
    #[serde(default)]
    max_chars: Option<usize>,
    /// Skip live git status in the pack.
    #[serde(default)]
    no_status: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RepoIdArgs {
    /// Repo id from poly.toml (e.g. "app", "core").
    repo: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PlanArgs {
    /// Free-text task query (e.g. "identity oauth").
    #[serde(default)]
    query: Option<String>,
    /// Comma-separated repo ids.
    #[serde(default)]
    repos: Option<String>,
    /// Comma-separated tags.
    #[serde(default)]
    tags: Option<String>,
    /// Filter by role.
    #[serde(default)]
    role: Option<String>,
    /// Skip expanding depends_on.
    #[serde(default)]
    no_deps: Option<bool>,
    /// Output format: prompt (default), markdown, or json.
    #[serde(default)]
    format: Option<String>,
    /// Skip live git status.
    #[serde(default)]
    no_status: Option<bool>,
}

// ── Tools ───────────────────────────────────────────────────────────────────

#[tool_router]
impl PolyMcp {
    #[tool(
        name = "list_repos",
        description = "List all repositories in the poly multi-repo workspace (id, path, role, tags, exists, is_git)."
    )]
    async fn list_repos(&self, _args: Parameters<EmptyArgs>) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        let rows: Vec<_> = ws
            .repos
            .iter()
            .map(|repo| {
                let path = ws.repo_path(repo);
                let exists = path.exists();
                serde_json::json!({
                    "id": repo.id,
                    "path": path,
                    "role": repo.role,
                    "tags": repo.tags,
                    "description": repo.description,
                    "depends_on": repo.depends_on,
                    "exists": exists,
                    "is_git": exists && path.join(".git").exists(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "workspace": ws.name,
            "root": ws.root,
            "repos": rows,
        });
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    #[tool(
        name = "status",
        description = "Git status across poly workspace repos: branch, dirty, ahead/behind, last subject."
    )]
    async fn status(&self, Parameters(args): Parameters<StatusArgs>) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        let filter = parse_csv(args.repos.as_deref());
        let report = status::collect_status(&ws, filter.as_deref(), args.fetch.unwrap_or(false));
        Ok(serde_json::to_string_pretty(&report).unwrap_or_default())
    }

    #[tool(
        name = "build_context",
        description = "Build a cross-repo context pack for a task (always-docs + selected repo AGENTS/README + status). Prefer format=prompt for agent consumption."
    )]
    async fn build_context(
        &self,
        Parameters(args): Parameters<CtxArgs>,
    ) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        let pack = context::build_context(
            &ws,
            args.query.as_deref(),
            parse_csv(args.repos.as_deref()).as_deref(),
            parse_csv(args.tags.as_deref()).as_deref(),
            args.role.as_deref(),
            args.max_chars,
            args.no_status.unwrap_or(false),
        )
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let format = args.format.as_deref().unwrap_or("prompt");
        match format {
            "json" => Ok(serde_json::to_string_pretty(&pack).unwrap_or_default()),
            "markdown" | "md" => Ok(context::format_markdown(&pack)),
            _ => Ok(context::format_prompt(&pack)),
        }
    }

    #[tool(
        name = "repo_path",
        description = "Return the absolute filesystem path of a poly repo by id."
    )]
    async fn repo_path(&self, Parameters(args): Parameters<RepoIdArgs>) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        let repo = ws
            .repos
            .iter()
            .find(|r| r.id == args.repo)
            .ok_or_else(|| McpError::invalid_params(format!("unknown repo id '{}'", args.repo), None))?;
        let path = ws.repo_path(repo);
        if !path.exists() {
            return Err(McpError::invalid_params(
                format!("repo '{}' path does not exist: {}", args.repo, path.display()),
                None,
            ));
        }
        Ok(path.display().to_string())
    }

    #[tool(
        name = "workspace_root",
        description = "Return the poly workspace root directory (directory containing poly.toml)."
    )]
    async fn workspace_root(&self, _args: Parameters<EmptyArgs>) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        Ok(ws.root.display().to_string())
    }

    #[tool(
        name = "plan",
        description = "Suggest which repos to touch for a task and the depends_on execution order. Call before multi-repo work; then build_context with the ordered repos."
    )]
    async fn plan_work(&self, Parameters(args): Parameters<PlanArgs>) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        let work = plan::build_plan(
            &ws,
            args.query.as_deref(),
            parse_csv(args.repos.as_deref()).as_deref(),
            parse_csv(args.tags.as_deref()).as_deref(),
            args.role.as_deref(),
            !args.no_deps.unwrap_or(false),
            args.no_status.unwrap_or(false),
        )
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let format = args.format.as_deref().unwrap_or("prompt");
        match format {
            "json" => Ok(serde_json::to_string_pretty(&work).unwrap_or_default()),
            "markdown" | "md" => Ok(plan::format_markdown(&work)),
            _ => Ok(plan::format_prompt(&work)),
        }
    }
}

#[tool_handler(
    name = "poly",
    version = "0.4.0",
    instructions = "poly exposes multi-repo workspace awareness. Workflow: plan (repo order) → build_context (format=prompt) → work only in selected repos. Scope edits correctly; commit only in the product repo. Meta/docs are context. Use repo_path before shell. Mutation stays on the poly CLI (exec/run), not MCP."
)]
impl ServerHandler for PolyMcp {}

fn parse_csv(s: Option<&str>) -> Option<Vec<String>> {
    s.map(|v| {
        v.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
            .collect()
    })
    .filter(|v: &Vec<String>| !v.is_empty())
}

// Silence unused Arc if macros need Sync — PolyMcp is Clone.
#[allow(dead_code)]
fn _assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<PolyMcp>();
    let _ = Arc::new(());
}
