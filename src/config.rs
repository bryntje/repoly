use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const DEFAULT_MAX_CHARS: usize = 48_000;
pub const DEFAULT_CONTEXT_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md", "README.md"];

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("workspace not found (no repoly.toml)")]
    NotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unsupported schema_version {0} (supported: 1)")]
    UnsupportedSchema(u32),
    #[error("{0}")]
    Validation(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepolyFile {
    pub schema_version: u32,
    pub workspace: WorkspaceSection,
    #[serde(default)]
    pub context: ContextSection,
    /// Optional workspace-level safety defaults (MCP session flags still win).
    #[serde(default)]
    pub policy: PolicySection,
    /// Optional ranking / synonym tuning for plan & ctx.
    #[serde(default)]
    pub ranking: RankingSection,
    #[serde(default)]
    pub repos: Vec<RepoEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkspaceSection {
    pub name: String,
    #[serde(default)]
    pub root: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ContextSection {
    /// Supports both old string syntax and new rich [[always]] syntax.
    #[serde(default)]
    pub always: Vec<AlwaysDoc>,
    #[serde(default)]
    pub status_doc: Option<String>,
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// Hard cap on total always-doc bytes (optional). Applied on top of the
    /// work-mode always budget (max_chars minus repo reserve).
    #[serde(default)]
    pub always_max_chars: Option<usize>,
    /// Percent of `max_chars` reserved for selected-repo files in work mode
    /// (query/repos/tags/role). Default 40 when unset. Clamped 10–80.
    #[serde(default)]
    pub repo_reserve_pct: Option<u8>,
}

/// Rich always-doc entry (supports both string and table form in TOML).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AlwaysDoc {
    /// Simple path (backward compatible)
    Path(String),
    /// Rich entry with optional tags, sections and priority
    Rich {
        path: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        sections: Vec<String>,
        #[serde(default)]
        priority: Option<u8>,
    },
}

impl AlwaysDoc {
    pub fn path(&self) -> &str {
        match self {
            AlwaysDoc::Path(p) => p,
            AlwaysDoc::Rich { path, .. } => path,
        }
    }

    pub fn tags(&self) -> &[String] {
        match self {
            AlwaysDoc::Path(_) => &[],
            AlwaysDoc::Rich { tags, .. } => tags,
        }
    }

    pub fn priority(&self) -> u8 {
        match self {
            AlwaysDoc::Path(_) => 50,
            AlwaysDoc::Rich { priority, .. } => priority.unwrap_or(50),
        }
    }

    pub fn sections(&self) -> &[String] {
        match self {
            AlwaysDoc::Path(_) => &[],
            AlwaysDoc::Rich { sections, .. } => sections,
        }
    }
}

/// Default share of the pack budget reserved for selected repo context files.
pub const DEFAULT_REPO_RESERVE_PCT: u8 = 40;

/// Optional `[policy]` in repoly.toml — generic safety knobs for any polyrepo.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolicySection {
    /// Glob patterns (relative path style) skipped when packing context.
    #[serde(default)]
    pub skip_globs: Vec<String>,
    /// Keep built-in secret filename heuristics (default true).
    #[serde(default = "default_true")]
    pub use_builtin_secret_filters: bool,
    /// Suggested MCP capture timeout when host does not pass a flag.
    #[serde(default)]
    pub exec_timeout_secs: Option<u64>,
    /// Suggested MCP max stdout/stderr bytes when host does not pass a flag.
    #[serde(default)]
    pub exec_max_output_bytes: Option<usize>,
    /// Optional audit log path (relative to workspace root or absolute).
    #[serde(default)]
    pub audit_log: Option<String>,
}

impl Default for PolicySection {
    fn default() -> Self {
        Self {
            skip_globs: Vec::new(),
            use_builtin_secret_filters: true,
            exec_timeout_secs: None,
            exec_max_output_bytes: None,
            audit_log: None,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Optional `[ranking]` — workspace-specific synonym groups for plan/ctx queries.
///
/// Each group is a list of equivalent terms: if the query hits any member, all
/// members are used as match variants (merged with built-in expand_token).
///
/// ```toml
/// [ranking]
/// synonym_groups = [
///   ["reflection", "reflections", "growth", "checkin"],
///   ["billing", "invoice", "stripe"],
/// ]
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RankingSection {
    #[serde(default)]
    pub synonym_groups: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepoEntry {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub context_files: Option<Vec<String>>,
}

/// Fully resolved workspace ready for commands.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub name: String,
    pub root: PathBuf,
    #[allow(dead_code)]
    pub config_path: PathBuf,
    pub context: ContextSection,
    pub policy: PolicySection,
    pub ranking: RankingSection,
    pub repos: Vec<RepoEntry>,
}

impl Workspace {
    pub fn repo_path(&self, repo: &RepoEntry) -> PathBuf {
        let p = Path::new(&repo.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }

    pub fn max_chars(&self) -> usize {
        self.context.max_chars.unwrap_or(DEFAULT_MAX_CHARS)
    }

    /// Repo-file reserve percentage for work-mode context packs.
    pub fn repo_reserve_pct(&self) -> u8 {
        self.context
            .repo_reserve_pct
            .unwrap_or(DEFAULT_REPO_RESERVE_PCT)
            .clamp(10, 80)
    }

    /// Soft validation. Returns warnings. Hard errors are returned via Result on load.
    pub fn validate(&self, _strict: bool) -> Vec<String> {
        let mut warnings = Vec::new();
        let ids: HashSet<_> = self.repos.iter().map(|r| r.id.as_str()).collect();
        for repo in &self.repos {
            for dep in &repo.depends_on {
                if !ids.contains(dep.as_str()) {
                    warnings.push(format!(
                        "repo '{}': depends_on '{}' does not exist",
                        repo.id, dep
                    ));
                }
            }
        }
        warnings
    }
}

pub fn find_config() -> Result<PathBuf, ConfigError> {
    if let Ok(p) = env::var("REPOLY_CONFIG") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
        return Err(ConfigError::NotFound);
    }

    let mut dir = env::current_dir()?;
    loop {
        let candidate = dir.join("repoly.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        let nested = dir.join(".repoly").join("repoly.toml");
        if nested.is_file() {
            return Ok(nested);
        }
        // Legacy paths from early "poly" naming (pre-public)
        let legacy = dir.join("poly.toml");
        if legacy.is_file() {
            return Ok(legacy);
        }
        let legacy_nested = dir.join(".poly").join("poly.toml");
        if legacy_nested.is_file() {
            return Ok(legacy_nested);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(ConfigError::NotFound)
}

pub fn load_config(path: &Path) -> Result<Workspace, ConfigError> {
    let raw = fs::read_to_string(path)?;
    let file: RepolyFile = toml::from_str(&raw).map_err(|e| ConfigError::Parse(e.to_string()))?;

    if file.schema_version != 1 {
        return Err(ConfigError::UnsupportedSchema(file.schema_version));
    }

    validate_hard(&file)?;

    let config_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let root = if let Some(r) = &file.workspace.root {
        let p = Path::new(r);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            config_dir.join(p)
        }
    } else {
        config_dir
    };

    Ok(Workspace {
        name: file.workspace.name,
        root: root.canonicalize().unwrap_or(root),
        config_path: path.to_path_buf(),
        context: file.context,
        policy: file.policy,
        ranking: file.ranking,
        repos: file.repos,
    })
}

fn validate_hard(file: &RepolyFile) -> Result<(), ConfigError> {
    if file.workspace.name.trim().is_empty() {
        return Err(ConfigError::Validation(
            "workspace.name must not be empty".into(),
        ));
    }
    if file.repos.is_empty() {
        return Err(ConfigError::Validation(
            "at least one [[repos]] entry is required".into(),
        ));
    }

    let id_re = Regex::new(r"^[a-z][a-z0-9_-]*$").expect("regex");
    let mut seen = HashSet::new();
    for repo in &file.repos {
        if !id_re.is_match(&repo.id) {
            return Err(ConfigError::Validation(format!(
                "invalid repo id '{}': must match ^[a-z][a-z0-9_-]*$",
                repo.id
            )));
        }
        if !seen.insert(repo.id.clone()) {
            return Err(ConfigError::Validation(format!(
                "duplicate repo id '{}'",
                repo.id
            )));
        }
        if repo.path.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "repo '{}': path must not be empty",
                repo.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parses_minimal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("repoly.toml");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            r#"
schema_version = 1
[workspace]
name = "acme"
[[repos]]
id = "api"
path = "./api"
[[repos]]
id = "web"
path = "./web"
depends_on = ["api"]
"#
        )
        .unwrap();
        let ws = load_config(&path).unwrap();
        assert_eq!(ws.name, "acme");
        assert_eq!(ws.repos.len(), 2);
        assert!(ws.validate(false).is_empty());
    }

    #[test]
    fn rejects_duplicate_ids() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("repoly.toml");
        fs::write(
            &path,
            r#"
schema_version = 1
[workspace]
name = "x"
[[repos]]
id = "api"
path = "./a"
[[repos]]
id = "api"
path = "./b"
"#,
        )
        .unwrap();
        assert!(load_config(&path).is_err());
    }
}

#[allow(clippy::items_after_test_module)]
impl From<&str> for AlwaysDoc {
    fn from(s: &str) -> Self {
        AlwaysDoc::Path(s.to_string())
    }
}

impl From<String> for AlwaysDoc {
    fn from(s: String) -> Self {
        AlwaysDoc::Path(s)
    }
}
