//! MCP (Model Context Protocol) stdio server for repoly.
//!
//! Read-only tools by default. Optional `exec` when started with `--allow-exec`.

use crate::commit;
use crate::config::{find_config, load_config, Workspace};
use crate::context;
use crate::plan;
use crate::policy::{self, ExecBinPolicy};
use crate::run::{self, CaptureOpts};
use crate::status;
use anyhow::{Context as _, Result};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::IsTerminal;
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
    /// Optional argv[0] basename allowlist.
    pub exec_bin_allow: Option<Vec<String>>,
    /// Extra argv[0] basenames to deny (merged with defaults unless no_default_exec_deny).
    pub exec_bin_deny: Option<Vec<String>>,
    /// Skip built-in sensitive binary deny list.
    pub no_default_exec_deny: bool,
    /// Capture timeout (seconds). `Some(0)` = unlimited; `None` = use workspace [policy] if set.
    pub exec_timeout_secs: Option<u64>,
    /// Max bytes per stream. `Some(0)` = unlimited; `None` = use workspace [policy] if set.
    pub exec_max_output_bytes: Option<usize>,
    /// Audit log path from CLI (overrides [policy].audit_log).
    pub audit_log: Option<PathBuf>,
}

/// Run the MCP server on stdio until the client disconnects.
pub async fn serve(opts: McpOptions) -> Result<()> {
    // MCP must keep stdout clean (JSON-RPC only). If a human ran this in a
    // terminal, explain on stderr so the "frozen cursor" is less confusing.
    if std::io::stdin().is_terminal() {
        eprintln!("repoly mcp: listening for JSON-RPC on stdin (stdio transport).");
        eprintln!("  This is normal — an MCP host (Grok, Claude Code, Cursor) connects here.");
        eprintln!("  Interactive use: Ctrl-C to quit. Configure the host instead of running this by hand.");
        if opts.allow_exec {
            eprint!("  exec: enabled");
            if let Some(ref repos) = opts.exec_repos {
                eprintln!(" (repos: {})", repos.join(", "));
            } else {
                eprintln!(" (all workspace repos)");
            }
            let use_default = !opts.no_default_exec_deny;
            if use_default {
                eprintln!("  bin deny: default sensitive list (sudo, dd, …); override with --exec-bin-deny / --no-default-exec-deny");
            }
            if let Some(ref allow) = opts.exec_bin_allow {
                eprintln!("  bin allow: {}", allow.join(", "));
            }
            if let Some(ref deny) = opts.exec_bin_deny {
                eprintln!("  bin deny extra: {}", deny.join(", "));
            }
        } else {
            eprintln!("  exec: disabled (pass --allow-exec to enable mutation tools)");
        }
        if opts.allow_shell {
            eprintln!("  shell: requested (only works if no bin allow/deny policy is active)");
        }
        eprintln!("  docs: https://github.com/bryntje/repoly/blob/master/docs/mcp.md");
        eprintln!();
    }

    let server = RepolyMcp::new(opts)?;
    let service = server
        .serve(stdio())
        .await
        .context("starting MCP stdio transport")?;
    service.waiting().await.context("MCP server session")?;
    Ok(())
}

#[derive(Clone)]
pub struct RepolyMcp {
    // Used by #[tool_handler] / ToolRouter macros.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    /// Resolved config path (if known at startup).
    config_path: Option<PathBuf>,
    allow_exec: bool,
    /// Allowlist of repo ids for exec; None = all workspace repos when allow_exec.
    exec_repos: Option<HashSet<String>>,
    allow_shell: bool,
    /// Command basename policy (layer 2).
    bin_policy: ExecBinPolicy,
    /// Capture limits for exec/run (resolved flags + workspace policy).
    capture: CaptureOpts,
    /// Optional audit log path.
    audit_log: Option<PathBuf>,
}

impl RepolyMcp {
    pub fn new(opts: McpOptions) -> Result<Self> {
        let config_path = opts.config.clone().or_else(|| find_config().ok());
        let ws_policy = if let Some(ref p) = config_path {
            load_config(p)
                .with_context(|| format!("loading {}", p.display()))?
                .policy
        } else {
            Default::default()
        };

        if opts.exec_repos.is_some() && !opts.allow_exec {
            anyhow::bail!("--exec-repos requires --allow-exec");
        }
        if opts.allow_shell && !opts.allow_exec {
            anyhow::bail!("--allow-shell requires --allow-exec");
        }
        if (opts.exec_bin_allow.is_some()
            || opts.exec_bin_deny.is_some()
            || opts.no_default_exec_deny
            || opts.exec_timeout_secs.is_some()
            || opts.exec_max_output_bytes.is_some()
            || opts.audit_log.is_some())
            && !opts.allow_exec
        {
            // audit_log and resource limits only matter with exec; allow reading them only if exec
            // is on — except audit could log commits only with allow_exec anyway.
            if opts.exec_bin_allow.is_some()
                || opts.exec_bin_deny.is_some()
                || opts.no_default_exec_deny
            {
                anyhow::bail!("exec bin policy flags require --allow-exec");
            }
        }

        let exec_repos = opts.exec_repos.map(|v| v.into_iter().collect());

        // When exec is disabled, keep empty policy. When enabled, apply default deny
        // unless --no-default-exec-deny, plus optional allow/deny lists.
        let bin_policy = if opts.allow_exec {
            ExecBinPolicy::from_parts(
                opts.exec_bin_allow,
                opts.exec_bin_deny,
                !opts.no_default_exec_deny,
            )
        } else {
            ExecBinPolicy::empty()
        };

        // Flags win over [policy]; 0 means unlimited.
        let timeout_secs = match opts.exec_timeout_secs {
            Some(0) => None,
            Some(n) => Some(n),
            None => ws_policy.exec_timeout_secs.filter(|&n| n > 0),
        };
        let max_output_bytes = match opts.exec_max_output_bytes {
            Some(0) => None,
            Some(n) => Some(n),
            None => ws_policy.exec_max_output_bytes.filter(|&n| n > 0),
        };
        let capture = CaptureOpts {
            timeout_secs,
            max_output_bytes,
        };

        let audit_log = opts.audit_log.or_else(|| {
            ws_policy.audit_log.as_ref().map(|p| {
                let path = PathBuf::from(p);
                if path.is_absolute() {
                    path
                } else if let Some(ref cfg) = config_path {
                    cfg.parent().map(|d| d.join(&path)).unwrap_or(path)
                } else {
                    path
                }
            })
        });

        if opts.allow_shell && bin_policy.is_active() {
            eprintln!(
                "repoly mcp: warning: --allow-shell is set but bin policy is active \
                 (default deny and/or custom lists); shell=true tool calls will be rejected. \
                 Use argv commands, or pass --no-default-exec-deny and clear bin lists."
            );
        }

        Ok(Self {
            tool_router: Self::tool_router(),
            config_path,
            allow_exec: opts.allow_exec,
            exec_repos,
            allow_shell: opts.allow_shell,
            bin_policy,
            capture,
            audit_log,
        })
    }

    fn audit(&self, tool: &str, event: serde_json::Value) {
        let Some(ref path) = self.audit_log else {
            return;
        };
        let mut full = event;
        if let Some(obj) = full.as_object_mut() {
            obj.insert(
                "ts".into(),
                serde_json::json!(chrono::Utc::now().to_rfc3339()),
            );
            obj.insert("tool".into(), serde_json::json!(tool));
        }
        policy::append_audit_log(path, &full);
    }

    fn load_ws(&self) -> Result<Workspace, McpError> {
        let path = if let Some(ref p) = self.config_path {
            p.clone()
        } else {
            find_config().map_err(|e| {
                McpError::invalid_params(
                    format!(
                        "workspace not found ({e}); start repoly mcp from a directory with repoly.toml \
                         or set REPOLY_CONFIG / pass --config"
                    ),
                    None,
                )
            })?
        };
        load_config(&path).map_err(|e| {
            McpError::invalid_params(format!("failed to load {}: {e}", path.display()), None)
        })
    }

    fn assert_exec_enabled(&self) -> Result<(), McpError> {
        if !self.allow_exec {
            return Err(McpError::invalid_params(
                "exec/run/commit are disabled; restart with `repoly mcp --allow-exec` \
                 (optionally `--exec-repos a,b`, `--allow-shell`)",
                None,
            ));
        }
        Ok(())
    }

    fn assert_exec_allowed(&self, repo_id: &str) -> Result<(), McpError> {
        self.assert_exec_enabled()?;
        if let Some(ref allow) = self.exec_repos {
            if !allow.contains(repo_id) {
                let mut list: Vec<_> = allow.iter().cloned().collect();
                list.sort();
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

    fn resolve_shell_mode(&self, want_shell: bool) -> Result<run::LaunchMode, McpError> {
        if want_shell && !self.allow_shell {
            return Err(McpError::invalid_params(
                "shell mode is disabled; restart with `repoly mcp --allow-exec --allow-shell`",
                None,
            ));
        }
        self.bin_policy
            .check_shell_allowed(want_shell)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        Ok(run::LaunchMode::from_shell_flag(want_shell))
    }

    fn check_command_policy(&self, cmd: &[String], want_shell: bool) -> Result<(), McpError> {
        self.resolve_shell_mode(want_shell)?;
        if !want_shell {
            self.bin_policy
                .check_argv(cmd)
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
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
    /// Repo id from repoly.toml (e.g. "app", "core").
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
struct RunArgs {
    /// Comma-separated repo ids (at least one of repos/tags/role required).
    #[serde(default)]
    repos: Option<String>,
    /// Comma-separated tags; match any.
    #[serde(default)]
    tags: Option<String>,
    /// Filter by role.
    #[serde(default)]
    role: Option<String>,
    /// Command argv, e.g. ["git", "status", "-sb"].
    command: Vec<String>,
    /// Run via sh -c / cmd /C. Requires `--allow-shell`.
    #[serde(default)]
    shell: Option<bool>,
    /// Run repos in parallel (captured). Default false (sequential).
    #[serde(default)]
    parallel: Option<bool>,
    /// Continue after a failure (sequential mode). Default false.
    #[serde(default)]
    continue_on_error: Option<bool>,
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
impl RepolyMcp {
    #[tool(
        name = "list_repos",
        description = "List all repositories in the repoly multi-repo workspace (id, path, role, tags, exists, is_git)."
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
        description = "Git status across repoly workspace repos: branch, dirty, ahead/behind, last subject."
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
        description = "Return the absolute filesystem path of a repoly repo by id."
    )]
    async fn repo_path(
        &self,
        Parameters(args): Parameters<RepoIdArgs>,
    ) -> Result<String, McpError> {
        let ws = self.load_ws()?;
        let repo = ws.repos.iter().find(|r| r.id == args.repo).ok_or_else(|| {
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
        description = "Return the repoly workspace root directory (directory containing repoly.toml)."
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
        description = "Run a command in one repo's working directory. Requires --allow-exec. Prefer argv command=[\"git\",\"status\",\"-sb\"]; shell=true needs --allow-shell."
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
        self.check_command_policy(&args.command, want_shell)?;
        let mode = run::LaunchMode::from_shell_flag(want_shell);

        let ws = self.load_ws()?;
        let entry = run::resolve_repo(&ws, &args.repo)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        // Capture mode: MCP owns stdio; never inherit.
        let result = run::exec_capture_opts(&ws, entry, &args.command, mode, self.capture)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        self.audit(
            "exec",
            serde_json::json!({
                "repo": args.repo,
                "argv": args.command,
                "shell": want_shell,
                "exit_code": result.code(),
                "ok": result.success(),
                "timed_out": result.timed_out,
            }),
        );
        let out = serde_json::json!({
            "repo": result.repo_id,
            "path": result.path,
            "command": args.command,
            "shell": want_shell,
            "exit_code": result.code(),
            "success": result.success(),
            "timed_out": result.timed_out,
            "stdout_truncated": result.stdout_truncated,
            "stderr_truncated": result.stderr_truncated,
            "error": result.error,
            "stdout": result.stdout.unwrap_or_default(),
            "stderr": result.stderr.unwrap_or_default(),
        });
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    #[tool(
        name = "run",
        description = "Run the same command across multiple workspace repos (like repoly run). Requires --allow-exec. Must pass repos and/or tags and/or role. Each target must be on --exec-repos allowlist if set. Always captures output. Prefer sequential (parallel=false)."
    )]
    async fn run_cmd(&self, Parameters(args): Parameters<RunArgs>) -> Result<String, McpError> {
        self.assert_exec_enabled()?;
        if args.command.is_empty() {
            return Err(McpError::invalid_params(
                "command must be a non-empty argv array",
                None,
            ));
        }

        let want_shell = args.shell.unwrap_or(false);
        self.check_command_policy(&args.command, want_shell)?;
        let mode = run::LaunchMode::from_shell_flag(want_shell);
        let parallel = args.parallel.unwrap_or(false);
        let continue_on_error = args.continue_on_error.unwrap_or(false);

        let ws = self.load_ws()?;
        let selected = run::select_repos(
            &ws,
            parse_csv(args.repos.as_deref()).as_deref(),
            parse_csv(args.tags.as_deref()).as_deref(),
            args.role.as_deref(),
        )
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        // Enforce allowlist on every target
        for repo in &selected {
            self.assert_exec_allowed(&repo.id)?;
        }

        // MCP always captures with session limits. Parallel still sequential under limits
        // for predictable timeout/kill behaviour (same process-group constraints).
        let _ = parallel; // accepted for API compat; capture path is sequential with opts
        let mut results = Vec::new();
        for repo in &selected {
            let r = run::exec_capture_opts(&ws, repo, &args.command, mode, self.capture)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let ok = r.success();
            results.push(r);
            if !ok && !continue_on_error {
                break;
            }
        }

        let rows: Vec<_> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "repo": r.repo_id,
                    "path": r.path,
                    "exit_code": r.code(),
                    "success": r.success(),
                    "timed_out": r.timed_out,
                    "stdout_truncated": r.stdout_truncated,
                    "stderr_truncated": r.stderr_truncated,
                    "error": r.error,
                    "stdout": r.stdout.clone().unwrap_or_default(),
                    "stderr": r.stderr.clone().unwrap_or_default(),
                })
            })
            .collect();

        let all_ok = results.iter().all(|r| r.success());
        let any_ok = results.iter().any(|r| r.success());
        let summary = if all_ok {
            "ok"
        } else if any_ok {
            "partial"
        } else {
            "failed"
        };

        let out = serde_json::json!({
            "command": args.command,
            "shell": want_shell,
            "parallel": parallel,
            "summary": summary,
            "repos": selected.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            "results": rows,
        });
        self.audit(
            "run",
            serde_json::json!({
                "repos": selected.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
                "argv": args.command,
                "shell": want_shell,
                "summary": summary,
            }),
        );
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    #[tool(
        name = "commit",
        description = "Create a git commit in one workspace repo. Requires --allow-exec (and --exec-repos allowlist if set). Prefer this over exec for commits: safer defaults, no shell, pathspecs confined to repo root, skips when nothing staged unless all=true."
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

        self.audit(
            "commit",
            serde_json::json!({
                "repo": args.repo,
                "ok": result.success,
                "skipped": result.skipped,
                "commit_sha": result.commit_sha,
            }),
        );
        Ok(serde_json::to_string_pretty(&result).unwrap_or_default())
    }
}

#[tool_handler]
impl ServerHandler for RepolyMcp {
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
            let shell = if self.allow_shell && !self.bin_policy.is_active() {
                "shell=true allowed (sh -c; use sparingly)"
            } else if self.allow_shell && self.bin_policy.is_active() {
                "shell requested but blocked by bin policy"
            } else {
                "shell=false only (argv; safer)"
            };
            let bins = if let Some(ref allow) = self.bin_policy.allow {
                let mut list: Vec<_> = allow.iter().cloned().collect();
                list.sort();
                format!("bin allow: {}", list.join(", "))
            } else if !self.bin_policy.deny.is_empty() {
                format!(
                    "bin deny active ({} names; default sensitive list and/or custom)",
                    self.bin_policy.deny.len()
                )
            } else {
                "bin policy: none".into()
            };
            let limits = match (self.capture.timeout_secs, self.capture.max_output_bytes) {
                (None, None) => "no capture limits".into(),
                (t, m) => format!(
                    "timeout={}s max_output={}",
                    t.map(|n| n.to_string()).unwrap_or_else(|| "∞".into()),
                    m.map(|n| n.to_string()).unwrap_or_else(|| "∞".into())
                ),
            };
            let audit = if self.audit_log.is_some() {
                "audit=on"
            } else {
                "audit=off"
            };
            format!(
                "exec/run/commit ENABLED ({repos}; {shell}; {bins}; {limits}; {audit}). \
                 Commits confine pathspecs to each repo root."
            )
        } else {
            "exec/run/commit DISABLED (start with `repoly mcp --allow-exec`[, `--exec-repos a,b`][, bin policy flags])."
                .into()
        };

        let instructions = format!(
            "repoly multi-repo workspace tools. Workflow: plan → build_context(format=prompt) → edit only selected repos. \
Use run for the same command across multiple repos (requires repos/tags/role). Prefer commit for git commits. \
{exec_note} Prefer argv arrays over shell. Avoid force-push/rm without user intent."
        );

        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("repoly", env!("CARGO_PKG_VERSION")))
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
    check::<RepolyMcp>();
    let _ = Arc::new(());
}
