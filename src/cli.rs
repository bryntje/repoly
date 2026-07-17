use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "repoly",
    about = "Multi-repo workspace awareness for the terminal",
    long_about = "repoly declares multi-repo workspaces, shows cross-repo git status, \
and builds agent-ready context packs. Local-only, CLI-first, no IDE required."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Create a repoly.toml in the current directory
    Init {
        /// Import folders from a VS Code / Cursor .code-workspace file
        #[arg(long = "from-code-workspace", value_name = "FILE")]
        from_code_workspace: Option<PathBuf>,
        /// Overwrite existing repoly.toml
        #[arg(long)]
        force: bool,
    },
    /// Validate repoly.toml schema, ids, and paths
    Validate {
        /// Treat warnings (missing depends_on targets, missing always-docs) as errors
        #[arg(long)]
        strict: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// List repos in the workspace
    List {
        #[arg(long, value_enum, default_value_t = ListFormat::Table)]
        format: ListFormat,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Show git status across repos
    Status {
        /// Comma-separated repo ids
        #[arg(long)]
        repos: Option<String>,
        #[arg(long, value_enum, default_value_t = StatusFormat::Table)]
        format: StatusFormat,
        /// Run git fetch before computing ahead/behind (network)
        #[arg(long)]
        fetch: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Suggest which repos to touch and in which order (depends_on)
    ///
    /// Example: `repoly plan "identity oauth"`
    Plan {
        /// Free-text query for keyword selection
        query: Option<String>,
        /// Force-include repo ids (comma-separated)
        #[arg(long)]
        repos: Option<String>,
        /// Include repos matching any of these tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,
        /// Include repos with this role
        #[arg(long)]
        role: Option<String>,
        /// Do not expand transitive depends_on into the plan
        #[arg(long)]
        no_deps: bool,
        #[arg(long, value_enum, default_value_t = PlanFormat::Markdown)]
        format: PlanFormat,
        /// Skip live git status lines
        #[arg(long)]
        no_status: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Build a cross-repo context pack for humans or agents
    Ctx {
        /// Free-text query for keyword selection (optional)
        query: Option<String>,
        /// Force-include repo ids (comma-separated)
        #[arg(long)]
        repos: Option<String>,
        /// Include repos matching any of these tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,
        /// Include repos with this role
        #[arg(long)]
        role: Option<String>,
        #[arg(long, value_enum, default_value_t = CtxFormat::Markdown)]
        format: CtxFormat,
        /// Override context.max_chars
        #[arg(long)]
        max_chars: Option<usize>,
        /// Skip live git status in the pack
        #[arg(long)]
        no_status: bool,
        /// Also include transitive depends_on of matched repos
        #[arg(long)]
        with_deps: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Print the workspace root path
    Root {
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Print the absolute path of a repo (scripting)
    Path {
        /// Repo id
        repo: String,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Create a git commit in one or more workspace repos
    ///
    /// Examples:
    ///   `repoly commit app -m "fix oauth callback"`
    ///   `repoly commit app -m "wip" --all`
    ///   `repoly commit --repos core,app -m "chore: bump" --all`
    Commit {
        /// Single repo id (alternative to --repos/--tags/--role)
        repo: Option<String>,
        /// Commit message (required)
        #[arg(short = 'm', long)]
        message: String,
        /// `git add -A` before commit
        #[arg(long, short = 'a')]
        all: bool,
        /// Stage these pathspecs (relative to each repo) instead of --all
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Comma-separated repo ids
        #[arg(long)]
        repos: Option<String>,
        /// Include repos matching any of these tags
        #[arg(long)]
        tags: Option<String>,
        /// Include repos with this role
        #[arg(long)]
        role: Option<String>,
        /// Amend HEAD (use carefully)
        #[arg(long)]
        amend: bool,
        /// Allow empty commit
        #[arg(long)]
        allow_empty: bool,
        /// Skip hooks (--no-verify)
        #[arg(long)]
        no_verify: bool,
        /// Add Signed-off-by
        #[arg(long, short = 's')]
        signoff: bool,
        /// Print git actions without running them
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Run a command in one repo's working directory
    ///
    /// Example: `repoly exec app -- npm test`
    ///
    /// Shell pipes/redirects (explicit, less safe):
    ///   `repoly exec app --shell -- 'npm test && echo ok'`
    Exec {
        /// Repo id
        repo: String,
        /// Print the command without running it
        #[arg(long)]
        dry_run: bool,
        /// Run via `sh -c` / `cmd /C` (pipes, &&, globs). Off by default.
        #[arg(long)]
        shell: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
        /// Command and args (use `--` before flags meant for the child)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        cmd: Vec<String>,
    },
    /// Run a command across one or more repos
    ///
    /// Example: `repoly run --repos core,app -- git status -sb`
    ///
    /// Default is sequential with inherited stdio (interactive CLIs work).
    /// `--parallel` captures output per repo (better for batch git/npm).
    Run {
        /// Comma-separated repo ids
        #[arg(long)]
        repos: Option<String>,
        /// Include repos matching any of these tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,
        /// Include repos with this role
        #[arg(long)]
        role: Option<String>,
        /// Run matching repos in parallel (captures stdout/stderr)
        #[arg(long)]
        parallel: bool,
        /// Do not stop at the first failure (sequential mode)
        #[arg(long)]
        continue_on_error: bool,
        /// Print commands without running them
        #[arg(long)]
        dry_run: bool,
        /// Run via `sh -c` / `cmd /C` (pipes, &&, globs). Off by default.
        #[arg(long)]
        shell: bool,
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
        /// Command and args
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        cmd: Vec<String>,
    },
    /// Start an MCP stdio server (for Grok, Claude Code, Cursor, …)
    ///
    /// Speaks JSON-RPC on stdin/stdout and **blocks until the client disconnects**.
    /// Do not run this by hand for normal use — configure your MCP host to spawn it.
    /// If stdin is a TTY, a short hint is printed on stderr.
    ///
    /// Read-only tools by default. Enable command execution with `--allow-exec`.
    /// Shell form for exec requires an extra `--allow-shell`.
    ///
    /// ```toml
    /// [mcp_servers.repoly]
    /// command = "repoly"
    /// args = ["mcp", "--allow-exec", "--exec-repos", "core,app"]
    /// env = { REPOLY_CONFIG = "/path/to/repoly.toml" }
    /// ```
    Mcp {
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
        /// Expose the `exec` / `run` / `commit` tools
        #[arg(long)]
        allow_exec: bool,
        /// Comma-separated repo ids that exec/run/commit may target (requires --allow-exec)
        #[arg(long, value_name = "IDS")]
        exec_repos: Option<String>,
        /// Allow MCP exec/run with shell=true (`sh -c`). Requires --allow-exec.
        #[arg(long)]
        allow_shell: bool,
    },
    /// Print version
    Version,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum ListFormat {
    #[default]
    Table,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum StatusFormat {
    #[default]
    Table,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum CtxFormat {
    #[default]
    Markdown,
    Prompt,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum PlanFormat {
    #[default]
    Markdown,
    Prompt,
    Json,
}
