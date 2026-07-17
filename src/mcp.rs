//! MCP (Model Context Protocol) stdio server for poly.
//!
//! Read-only tools by default. Optional `exec` when started with `--allow-exec`.

use crate::commit;
use crate::config::{find_config, load_config, Workspace};
use crate::context;
use crate::plan;
use crate::run;
use crate::status;
use anyhow::{Context as _, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Options for the MCP server (CLI / env).
#[derive(Debug, Clone, Default)]
pub struct McpOptions {
    pub config: Option<PathBuf>,
    /// Enable the `exec` tool (runs commands in a repo cwd).
    pub allow_exec: bool,
    /// If set, `exec` may only target these repo ids.
    pub exec_repos: Option<Vec<String>>,
    /// Allow `exec` with shell=true (`sh -c`). Requires allow_exec.
    pub allow_shell: bool,
}

/// Run the MCP server on stdio until the client disconnects.
pub async fn serve(opts: McpOptions) -> Result<()> {
    let server = PolyMcp::new(opts)?;
    let service = server
        .serve(stdio())
        .await
        .context("starting MCP stdio transport")?;
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
    allow_exec: bool,
    /// Allowlist of repo ids for exec; None = all workspace repos when allow_exec.
    exec_repos: Option<HashSet<String>>,
    allow_shell: bool,
}

impl PolyMcp {
    pub fn new(opts: McpOptions) -> Result<Self> {
        if let Some(ref p) = opts.config {
            let _ = load_config(p).with_context(|| format!("loading {}", p.display()))?;
        } else if let Ok(p) = find_config() {
            let _ = load_config(&p).with_context(|| format!("loading {}", p.display()))?;
        }

        if opts.exec_repos.is_some() && !opts.allow_exec {
            anyhow::bail!("--exec-repos requires --allow-exec");
        }
        if opts.allow_shell && !opts.allow_exec {
            anyhow::bail!("--allow-shell requires --allow-exec");
        }

        let exec_repos = opts.exec_repos.map(|v| v.into_iter().collect());

        Ok(Self {
            tool_router: Self::tool_router(),
            config_path: opts.config.or_else(|| find_config().ok()),
            allow_exec: opts.allow_exec,
            exec_repos,
            allow_shell: opts.allow_shell,
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

    fn assert_exec_allowed(&self, repo_id: &str) -> Result<(), McpError> {
        if !self.allow_exec {
            return Err(McpError::invalid_params(
                "exec is disabled; restart the server with `poly mcp --allow-exec` \
                 (optionally `--exec-repos a,b` to restrict targets)",
                None,
            ));
        }
        if let Some(ref allow) = self.exec_repos {
            if !allow.contains(repo_id) {
                let list: Vec<_> = allow.iter().cloned().collect();
                return Err(McpError::invalid_params(
                    format!(
                        "repo '{repo_id}' is not in the exec allowlist (allowed: {})",
                        list.join(", ")
                    ),
                    None,
                ));
            }
        }
        Ok(())
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
    /// Also include transitive depends_on of matched repos.
    #[serde(default)]
    with_deps: Option<bool>,
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

#[derive(Debug, Deserialize, JsonSchema)]
struct ExecArgs {
    /// Repo id to run in (must be allowed when --exec-repos is set).
    repo: String,
    /// Command argv as separate strings, e.g. ["git", "status", "-sb"].
    /// First element is the program; remaining are args. No shell expansion unless shell=true.
    command: Vec<String>,
    /// Run via sh -c / cmd /C. Requires server `--allow-shell`. Prefer false.
    #[serde(default)]
    shell: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CommitArgs {
    /// Repo id to commit in (must be allowlisted when --exec-repos is set).
    repo: String,
    /// Commit message (required, non-empty).
    message: String,
    /// Run `git add -A` before commit.
    #[serde(default)]
    all: Option<bool>,
    /// Pathspecs to stage (relative to repo root). Do not combine with all=true.
    #[serde(default)]
    paths: Option<Vec<String>>,
    /// Amend HEAD.
    #[serde(default)]
    amend: Option<bool>,
    /// Allow empty commit.
    #[serde(default)]
    allow_empty: Option<bool>,
    /// Skip hooks.
    #[serde(default)]
    no_verify: Option<bool>,
    /// Add Signed-off-by.
    #[serde(default)]
    signoff: Option<bool>,
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
            args.with_deps.unwrap_or(false),
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
            .ok_or_else(|| {
                McpError::invalid_params(format!("unknown repo id '{}'", args.repo), None)
            })?;
        let path = ws.repo_path(repo);
        if !path.exists() {
            return Err(McpError::invalid_params(
                format!(
                    "repo '{}' path does not exist: {}",
                    args.repo,
                    path.display()
                ),
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

    #[tool(
        name = "exec",
        description = "Run a command in one repo's working directory (no shell). Requires the server to be started with --allow-exec. Optional --exec-repos restricts targets. Prefer argv form: command=[\"git\",\"status\",\"-sb\"]."
    )]
    async fn exec_cmd(&self, Parameters(args): Parameters<ExecArgs>) -> Result<String, McpError> {
        self.assert_exec_allowed(&args.repo)?;
        if args.command.is_empty() {
            return Err(McpError::invalid_params(
                "command must be a non-empty argv array (e.g. [\"git\", \"status\"])",
                None,
            ));
        }

        let want_shell = args.shell.unwrap_or(false);
        if want_shell && !self.allow_shell {
            return Err(McpError::invalid_params(
                "shell exec is disabled; restart with `poly mcp --allow-exec --allow-shell`",
                None,
            ));
        }
        let mode = run::LaunchMode::from_shell_flag(want_shell);

        let ws = self.load_ws()?;
        let entry = run::resolve_repo(&ws, &args.repo)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        // Capture mode: MCP owns stdio; never inherit.
        let result = run::exec_capture(&ws, entry, &args.command, mode)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let out = serde_json::json!({
            "repo": result.repo_id,
            "path": result.path,
            "command": args.command,
            "shell": want_shell,
            "exit_code": result.code(),
            "success": result.success(),
            "error": result.error,
            "stdout": result.stdout.unwrap_or_default(),
            "stderr": result.stderr.unwrap_or_default(),
        });
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    #[tool(
        name = "commit",
        description = "Create a git commit in one workspace repo. Requires --allow-exec (and --exec-repos allowlist if set). Prefer this over exec for commits: safer defaults, no shell, skips when nothing staged unless all=true."
    )]
    async fn commit_cmd(
        &self,
        Parameters(args): Parameters<CommitArgs>,
    ) -> Result<String, McpError> {
        self.assert_exec_allowed(&args.repo)?;
        let ws = self.load_ws()?;
        let entry = run::resolve_repo(&ws, &args.repo)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let opts = commit::CommitOpts {
            message: args.message,
            all: args.all.unwrap_or(false),
            paths: args.paths.unwrap_or_default(),
            amend: args.amend.unwrap_or(false),
            allow_empty: args.allow_empty.unwrap_or(false),
            no_verify: args.no_verify.unwrap_or(false),
            dry_run: false,
            signoff: args.signoff.unwrap_or(false),
        };

        let result = commit::commit_one(&ws, entry, &opts)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }
}

#[tool_handler]
impl ServerHandler for PolyMcp {
    fn get_info(&self) -> ServerInfo {
        let exec_note = if self.allow_exec {
            let repos = match &self.exec_repos {
                Some(set) => {
                    let mut ids: Vec<_> = set.iter().cloned().collect();
                    ids.sort();
                    format!("allowlisted repos: {}", ids.join(", "))
                }
                None => "all workspace repos".into(),
            };
            let shell = if self.allow_shell {
                "shell=true allowed (sh -c; use sparingly)"
            } else {
                "shell=false only (argv; safer)"
            };
            format!("exec ENABLED ({repos}; {shell}).")
        } else {
            "exec DISABLED (start with `poly mcp --allow-exec`[, `--exec-repos a,b`][, `--allow-shell`])."
                .into()
        };

        let instructions = format!(
            "poly multi-repo workspace tools. Workflow: plan → build_context(format=prompt) → edit only selected repos. \
Prefer the commit tool for git commits (not raw exec). Commit only in the correct product repo; meta/docs are context. \
{exec_note} Prefer argv arrays over shell. Avoid force-push/rm without user intent."
        );

        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new("poly", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions)
    }
}

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

#[allow(dead_code)]
fn _assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<PolyMcp>();
    let _ = Arc::new(());
}
