//! Shared safety policy helpers for repoly (layer 2).
//!
//! Generic: no product- or workspace-specific repo ids.
//! Path confinement, command basename policy, and defaults live here.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Built-in basenames rejected under MCP exec unless the operator disables default deny.
/// Sensitive system tools only — not everyday build tools, and not `rm` (too common).
pub const DEFAULT_EXEC_BIN_DENY: &[&str] = &[
    "sudo",
    "doas",
    "su",
    "pkexec",
    "dd",
    "mkfs",
    "mkfs.ext4",
    "mkfs.xfs",
    "mkfs.vfat",
    "diskutil",
    "diskpart",
    "format",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
];

/// Resolved command policy for a session / workspace.
#[derive(Debug, Clone, Default)]
pub struct ExecBinPolicy {
    /// If non-empty, argv[0] basename must be in this set.
    pub allow: Option<HashSet<String>>,
    /// argv[0] basename must not be in this set.
    pub deny: HashSet<String>,
}

impl ExecBinPolicy {
    pub fn empty() -> Self {
        Self::default()
    }

    /// MCP default: built-in sensitive deny list only.
    pub fn mcp_default() -> Self {
        Self {
            allow: None,
            deny: DEFAULT_EXEC_BIN_DENY
                .iter()
                .map(|s| normalize_bin(s))
                .collect(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.allow.as_ref().is_some_and(|a| !a.is_empty()) || !self.deny.is_empty()
    }

    /// Merge user allow/deny CSV (or lists). `use_default_deny` adds [`DEFAULT_EXEC_BIN_DENY`].
    pub fn from_parts(
        allow: Option<Vec<String>>,
        deny: Option<Vec<String>>,
        use_default_deny: bool,
    ) -> Self {
        let allow = allow.and_then(|v| {
            let set: HashSet<_> = v
                .into_iter()
                .map(|s| normalize_bin(&s))
                .filter(|s| !s.is_empty())
                .collect();
            if set.is_empty() {
                None
            } else {
                Some(set)
            }
        });

        let mut deny_set = HashSet::new();
        if use_default_deny {
            for s in DEFAULT_EXEC_BIN_DENY {
                deny_set.insert(normalize_bin(s));
            }
        }
        if let Some(extra) = deny {
            for s in extra {
                let n = normalize_bin(&s);
                if !n.is_empty() {
                    deny_set.insert(n);
                }
            }
        }

        Self {
            allow,
            deny: deny_set,
        }
    }

    /// Validate argv (not shell). Returns Ok(basename) or an error message.
    pub fn check_argv(&self, cmd: &[String]) -> Result<String> {
        if cmd.is_empty() {
            bail!("command must be a non-empty argv array");
        }
        let program = normalize_bin(&cmd[0]);
        if program.is_empty() {
            bail!("command program name is empty");
        }
        if let Some(ref allow) = self.allow {
            if !allow.contains(&program) {
                let mut list: Vec<_> = allow.iter().cloned().collect();
                list.sort();
                bail!(
                    "binary '{program}' is not in the exec allowlist (allowed: {})",
                    list.join(", ")
                );
            }
        }
        if self.deny.contains(&program) {
            bail!("binary '{program}' is denied by exec policy");
        }
        Ok(program)
    }

    /// Shell mode cannot be combined with an active bin policy (would bypass basename checks).
    pub fn check_shell_allowed(&self, want_shell: bool) -> Result<()> {
        if want_shell && self.is_active() {
            bail!(
                "command policy requires argv mode; shell=true is rejected when \
                 exec bin allow/deny is active (including the default deny list). \
                 Use argv arrays, or clear bin policy / pass --no-default-exec-deny with no deny list"
            );
        }
        Ok(())
    }
}

/// Whether a file path should be skipped for context packing.
///
/// `relative` is a workspace- or repo-relative path using `/` separators.
/// Built-in secret filename heuristics apply when `use_builtin` is true.
pub fn should_skip_context_path(
    relative: &str,
    file_name: &str,
    skip_globs: &[String],
    use_builtin: bool,
) -> bool {
    if use_builtin && is_builtin_secret_name(file_name) {
        return true;
    }
    let rel = relative.replace('\\', "/");
    for g in skip_globs {
        if glob_match(g, &rel) || glob_match(g, file_name) {
            return true;
        }
    }
    false
}

pub fn is_builtin_secret_name(name: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with(".env")
        || name.contains("credential")
        || name.contains("secret")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name == "id_rsa"
        || name == "id_ed25519"
}

/// Simple glob match: `*`, `**`, `?`; case-insensitive path match.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    let re = glob_to_regex(&pattern);
    re.is_match(&path)
}

fn glob_to_regex(pattern: &str) -> Regex {
    // Cache is per-pattern via OnceLock map would be better; compile each call is fine for small lists.
    let mut re = String::from("(?i)^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                re.push_str(".*");
                i += 2;
                if i < chars.len() && chars[i] == '/' {
                    // `**/` can match zero directories
                    i += 1;
                    re.push_str("(?:.*/)?");
                }
            }
            '*' => {
                re.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                re.push_str("[^/]");
                i += 1;
            }
            c if ".+()[]{}^$|\\".contains(c) => {
                re.push('\\');
                re.push(c);
                i += 1;
            }
            c => {
                re.push(c);
                i += 1;
            }
        }
    }
    re.push('$');
    Regex::new(&re).unwrap_or_else(|_| Regex::new("^$").expect("empty"))
}

/// Append a JSON line to an audit log. Failures are soft (stderr only).
pub fn append_audit_log(path: &Path, event: &serde_json::Value) {
    use std::fs::OpenOptions;
    use std::io::Write;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{event}") {
                eprintln!("repoly: audit log write failed ({}): {e}", path.display());
            }
        }
        Err(e) => {
            eprintln!("repoly: audit log open failed ({}): {e}", path.display());
        }
    }
}

/// Lowercase basename of a program path (`/usr/bin/Git` → `git`).
pub fn normalize_bin(program: &str) -> String {
    let p = Path::new(program.trim());
    p.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program.trim())
        .to_ascii_lowercase()
}

/// Ensure `user_path` resolves strictly under `root` (no `..` escape, no absolute outside).
///
/// Used for `commit` pathspecs. Works for paths that do not exist yet by normalizing
/// components without requiring the final file to exist.
pub fn ensure_under_root(root: &Path, user_path: &Path) -> Result<PathBuf> {
    if user_path.as_os_str().is_empty() {
        bail!("path must not be empty");
    }

    let root = normalize_existing_dir(root)
        .with_context(|| format!("repo root not usable: {}", root.display()))?;

    // Absolute user paths: must still fall under root.
    let joined = if user_path.is_absolute() {
        user_path.to_path_buf()
    } else {
        root.join(user_path)
    };

    let normalized = normalize_path_lexically(&joined)?;
    let root_norm = normalize_path_lexically(&root)?;

    if !path_starts_with(&normalized, &root_norm) {
        bail!(
            "path '{}' escapes repository root '{}'",
            user_path.display(),
            root.display()
        );
    }

    // Prefer canonicalize when the path (or a prefix) exists, to catch symlink escapes.
    if let Ok(canon_root) = root.canonicalize() {
        if let Some(canon) = canonicalize_existing_prefix(&normalized) {
            if !path_starts_with(&canon, &canon_root) {
                bail!(
                    "path '{}' escapes repository root '{}' (after resolving symlinks)",
                    user_path.display(),
                    root.display()
                );
            }
            return Ok(canon);
        }
    }

    Ok(normalized)
}

fn normalize_existing_dir(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        path.canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()))
    } else {
        Ok(normalize_path_lexically(path)?)
    }
}

/// Lexical normalization: resolve `.` and `..` without touching the filesystem.
fn normalize_path_lexically(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    // `..` above root — keep as failure for relative escapes under root check
                    bail!("path escapes filesystem root via '..'");
                }
                // On Unix, popping RootDir leaves empty — re-add root
                if out.as_os_str().is_empty() && path.is_absolute() {
                    out.push(Component::RootDir.as_os_str());
                }
            }
            Component::Normal(c) => out.push(c),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::CurDir.as_os_str());
    }
    Ok(out)
}

fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    let path_c: Vec<_> = path.components().collect();
    let prefix_c: Vec<_> = prefix.components().collect();
    if prefix_c.len() > path_c.len() {
        return false;
    }
    path_c
        .iter()
        .zip(prefix_c.iter())
        .all(|(a, b)| a.as_os_str() == b.as_os_str())
}

/// Canonicalize the longest existing prefix, then append remaining components.
fn canonicalize_existing_prefix(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return path.canonicalize().ok();
    }
    let mut components: Vec<_> = path.components().collect();
    while !components.is_empty() {
        components.pop();
        let candidate: PathBuf = components.iter().collect();
        if candidate.as_os_str().is_empty() {
            break;
        }
        if candidate.exists() {
            let mut canon = candidate.canonicalize().ok()?;
            let full: Vec<_> = path.components().collect();
            for c in full.iter().skip(components.len()) {
                match c {
                    Component::Normal(n) => canon.push(n),
                    Component::CurDir => {}
                    Component::ParentDir => {
                        canon.pop();
                    }
                    _ => {}
                }
            }
            return Some(canon);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn normalize_bin_strips_path_and_case() {
        assert_eq!(normalize_bin("/usr/bin/Git"), "git");
        assert_eq!(normalize_bin("SUDO"), "sudo");
        assert_eq!(normalize_bin("cargo"), "cargo");
    }

    #[test]
    fn default_deny_blocks_sudo_allows_git() {
        let p = ExecBinPolicy::mcp_default();
        assert!(p.check_argv(&["git".into(), "status".into()]).is_ok());
        assert!(p.check_argv(&["sudo".into(), "ls".into()]).is_err());
        assert!(p.check_argv(&["dd".into(), "if=/dev/zero".into()]).is_err());
    }

    #[test]
    fn allowlist_and_deny_combine() {
        let p = ExecBinPolicy::from_parts(
            Some(vec!["git".into(), "sudo".into()]),
            Some(vec!["sudo".into()]),
            false,
        );
        assert!(p.check_argv(&["git".into()]).is_ok());
        assert!(p.check_argv(&["sudo".into()]).is_err());
        assert!(p.check_argv(&["rm".into()]).is_err());
    }

    #[test]
    fn shell_rejected_when_policy_active() {
        let p = ExecBinPolicy::mcp_default();
        assert!(p.check_shell_allowed(true).is_err());
        assert!(p.check_shell_allowed(false).is_ok());
        let empty = ExecBinPolicy::empty();
        assert!(empty.check_shell_allowed(true).is_ok());
    }

    #[test]
    fn path_under_root_ok() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("file.txt"), "x").unwrap();
        let p = ensure_under_root(root, Path::new("file.txt")).unwrap();
        assert!(p.ends_with("file.txt"));
    }

    #[test]
    fn path_dotdot_escape_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let err = ensure_under_root(&root, Path::new("../outside")).unwrap_err();
        assert!(
            err.to_string().contains("escapes"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn path_absolute_outside_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let outside = dir.path().join("secret");
        fs::write(&outside, "nope").unwrap();
        let err = ensure_under_root(&root, &outside).unwrap_err();
        assert!(err.to_string().contains("escapes"), "unexpected: {err}");
    }

    #[test]
    fn path_nested_relative_ok_even_if_missing() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let p = ensure_under_root(root, Path::new("src/new_file.rs")).unwrap();
        assert!(p.ends_with("src/new_file.rs") || p.ends_with("new_file.rs"));
    }

    #[test]
    fn path_empty_rejected() {
        let dir = tempdir().unwrap();
        assert!(ensure_under_root(dir.path(), Path::new("")).is_err());
    }

    #[test]
    fn skip_globs_and_builtin_secrets() {
        assert!(should_skip_context_path(".env", ".env", &[], true));
        assert!(!should_skip_context_path(
            "README.md",
            "README.md",
            &[],
            true
        ));
        assert!(should_skip_context_path(
            "config/local.pem",
            "local.pem",
            &["**/*.pem".into()],
            false
        ));
        assert!(glob_match("**/.npmrc", "pkg/.npmrc"));
        assert!(glob_match("*.key", "id.key"));
    }
}
