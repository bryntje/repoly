use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "poly",
    about = "Multi-repo workspace awareness for the terminal",
    long_about = "poly declares multi-repo workspaces, shows cross-repo git status, \
and builds agent-ready context packs. Local-only, CLI-first, no IDE required."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Create a poly.toml in the current directory
    Init {
        /// Import folders from a VS Code / Cursor .code-workspace file
        #[arg(long = "from-code-workspace", value_name = "FILE")]
        from_code_workspace: Option<PathBuf>,
        /// Overwrite existing poly.toml
        #[arg(long)]
        force: bool,
    },
    /// Validate poly.toml schema, ids, and paths
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
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
    },
    /// Print the workspace root path
    Root {
        #[arg(long, short = 'c')]
        config: Option<PathBuf>,
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
