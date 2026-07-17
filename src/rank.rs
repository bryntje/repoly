//! Query ranking for multi-repo selection (`ctx` / `plan`).

use crate::config::{RepoEntry, Workspace, DEFAULT_CONTEXT_FILES};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Max bytes of each context file used for content matching.
const SNIPPET_BYTES: usize = 12_000;

/// Expand a query token with lightweight synonyms (English polyrepo/product terms).
pub fn expand_token(token: &str) -> Vec<String> {
    let t = token.to_lowercase();
    let mut out = vec![t.clone()];
    let extra: &[&str] = match t.as_str() {
        "id" | "identity" | "innersync-id" | "innersync_id" => {
            &["identity", "id", "oauth", "profile", "users", "auth"]
        }
        "oauth" | "auth" | "login" | "signin" | "sso" => {
            // Keep synonyms tight — avoid global words like "session" that pollute ranking
            &["oauth", "auth", "login", "signin"]
        }
        "payment" | "payments" | "billing" | "checkout" | "mollie" | "premium" => {
            &["payments", "payment", "checkout", "mollie", "premium", "billing"]
        }
        "discord" | "bot" | "guild" => &["discord", "bot", "guild", "alphapy"],
        "agent" | "agents" | "hermit" | "hermes" => {
            &["agents", "agent", "hermit", "memory", "vault"]
        }
        "reflection" | "reflections" | "journal" => {
            &["reflections", "reflection", "journal", "encryption"]
        }
        "telemetry" | "metrics" | "ops" | "cockpit" => {
            &["telemetry", "metrics", "ops", "mind", "railway"]
        }
        "ui" | "style" | "styling" | "theme" | "design" => {
            &["ui", "styling", "theme", "frontend"]
        }
        "docs" | "documentation" | "readme" => &["docs", "documentation", "starlight"],
        "api" | "backend" | "core" => &["api", "backend", "core", "fastapi"],
        "frontend" | "web" | "app" | "dashboard" => {
            &["frontend", "web", "app", "dashboard", "next"]
        }
        "db" | "database" | "postgres" | "supabase" | "sql" => {
            &["supabase", "postgres", "database", "sql", "migration"]
        }
        _ => &[],
    };
    for e in extra {
        if !out.iter().any(|x| x == e) {
            out.push((*e).to_string());
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct RankedRepo<'a> {
    pub repo: &'a RepoEntry,
    pub score: i32,
    pub reasons: Vec<String>,
    /// How many original query tokens hit at least once (coverage).
    pub token_hits: usize,
}

/// Score all repos for a free-text query. Higher is better.
pub fn rank_repos<'a>(workspace: &'a Workspace, query: &str) -> Vec<RankedRepo<'a>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }

    let original_tokens: Vec<String> = q
        .split(|c: char| c.is_whitespace() || c == ',' || c == '/')
        .map(str::trim)
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect();
    if original_tokens.is_empty() {
        return Vec::new();
    }

    // Always-docs snippet (shared boost signal)
    let always_blob = load_always_blob(workspace);

    let mut ranked = Vec::new();
    for repo in &workspace.repos {
        let mut score = 0i32;
        let mut structured = 0i32; // id/role/tag/desc only
        let mut reasons = Vec::new();
        let mut tokens_hit: HashSet<usize> = HashSet::new();
        let mut structured_token_hits: HashSet<usize> = HashSet::new();

        let id = repo.id.to_lowercase();
        let role = repo.role.as_deref().unwrap_or("").to_lowercase();
        let desc = repo.description.as_deref().unwrap_or("").to_lowercase();
        let tags = repo.tags.join(" ").to_lowercase();
        let file_blob = load_repo_snippet(workspace, repo);

        for (ti, token) in original_tokens.iter().enumerate() {
            let variants = expand_token(token);
            let mut token_structured = 0i32;
            let mut token_file = 0i32;
            let mut token_reasons: Vec<String> = Vec::new();

            for v in &variants {
                let is_primary = v == token;

                if id == *v {
                    token_structured += if is_primary { 14 } else { 10 };
                    token_reasons.push(format!("id={id}"));
                } else if id.contains(v.as_str()) && v.len() >= 3 {
                    token_structured += if is_primary { 7 } else { 4 };
                    token_reasons.push(format!("id~{v}"));
                }
                if !role.is_empty() && role == *v {
                    token_structured += 6;
                    token_reasons.push(format!("role={role}"));
                } else if !role.is_empty() && role.contains(v.as_str()) && v.len() >= 3 {
                    token_structured += 3;
                    token_reasons.push(format!("role~{v}"));
                }
                if repo.tags.iter().any(|t| t.eq_ignore_ascii_case(v)) {
                    token_structured += if is_primary { 10 } else { 7 };
                    token_reasons.push(format!("tag={v}"));
                } else if tags.split(|c: char| c.is_whitespace() || c == '-' || c == '_')
                    .any(|tag| tag == v.as_str())
                {
                    token_structured += 4;
                    token_reasons.push(format!("tag~{v}"));
                }
                if desc.contains(v.as_str()) && v.len() >= 3 {
                    token_structured += if is_primary { 4 } else { 2 };
                    token_reasons.push(format!("desc~{v}"));
                }

                // File content: only primary token (not full synonym fan-out) to limit noise
                if is_primary && !file_blob.is_empty() && file_blob.contains(v.as_str()) {
                    token_file += 2;
                    token_reasons.push(format!("file~{v}"));
                }
            }

            // Always-docs: ignore for scoring (too global); kept loaded for future use
            let _ = &always_blob;

            if token_structured > 0 {
                structured_token_hits.insert(ti);
                tokens_hit.insert(ti);
                structured += token_structured;
                score += token_structured;
                reasons.extend(token_reasons.iter().cloned());
            } else if token_file > 0 {
                // File-only: count only if we already have structured hits elsewhere, applied later
                tokens_hit.insert(ti);
                score += token_file;
                reasons.extend(token_reasons);
            }
        }

        // Multi-token coverage on structured hits only
        let coverage = structured_token_hits.len();
        if coverage > 1 {
            let bonus = (coverage as i32 - 1) * 8;
            score += bonus;
            structured += bonus;
            reasons.push(format!("coverage={coverage}/{}", original_tokens.len()));
        }

        // Drop pure file-noise repos when structured score is zero
        if structured == 0 {
            continue;
        }

        if score > 0 {
            reasons.sort();
            reasons.dedup();
            if reasons.len() > 12 {
                reasons.truncate(12);
                reasons.push("…".into());
            }
            ranked.push(RankedRepo {
                repo,
                score,
                reasons,
                token_hits: coverage.max(1),
            });
        }
    }

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.token_hits.cmp(&a.token_hits))
            .then_with(|| a.repo.id.cmp(&b.repo.id))
    });

    // Keep strong matches relative to the top hit; never raise floor above top.
    if let Some(top) = ranked.first().map(|r| r.score) {
        let floor = (top * 40 / 100).max(1).min(top); // keep >= 40% of top score
        ranked.retain(|r| r.score >= floor);
    }

    ranked
}

fn load_always_blob(workspace: &Workspace) -> String {
    let mut s = String::new();
    for rel in &workspace.context.always {
        let p = workspace.root.join(rel);
        if let Some(chunk) = read_snippet(&p, 6_000) {
            s.push_str(&chunk);
            s.push('\n');
        }
    }
    if let Some(rel) = &workspace.context.status_doc {
        let p = workspace.root.join(rel);
        if let Some(chunk) = read_snippet(&p, 3_000) {
            s.push_str(&chunk);
        }
    }
    s.to_lowercase()
}

fn load_repo_snippet(workspace: &Workspace, repo: &RepoEntry) -> String {
    let root = workspace.repo_path(repo);
    let files = repo
        .context_files
        .as_ref()
        .map(|v| v.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .unwrap_or_else(|| DEFAULT_CONTEXT_FILES.to_vec());
    let mut s = String::new();
    for f in files {
        let p = root.join(f);
        if is_secret_name(f) {
            continue;
        }
        if let Some(chunk) = read_snippet(&p, SNIPPET_BYTES) {
            s.push_str(&chunk);
            s.push('\n');
            // One solid file is enough for ranking speed
            if s.len() > 4_000 {
                break;
            }
        }
    }
    s.to_lowercase()
}

fn read_snippet(path: &Path, max_bytes: usize) -> Option<String> {
    let data = fs::read(path).ok()?;
    let take = data.len().min(max_bytes);
    let slice = &data[..take];
    // lossy is fine for ranking
    Some(String::from_utf8_lossy(slice).into_owned())
}

fn is_secret_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with(".env") || n.contains("secret") || n.contains("credential")
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
            config_path: PathBuf::from("/tmp/t/poly.toml"),
            context: ContextSection::default(),
            repos: vec![
                RepoEntry {
                    id: "core".into(),
                    path: "./core".into(),
                    role: Some("api".into()),
                    tags: vec!["identity".into(), "oauth".into(), "payments".into()],
                    depends_on: vec![],
                    description: Some("Core API payments identity".into()),
                    context_files: None,
                },
                RepoEntry {
                    id: "app".into(),
                    path: "./app".into(),
                    role: Some("frontend".into()),
                    tags: vec!["oauth".into(), "reflections".into()],
                    depends_on: vec!["core".into()],
                    description: Some("User app".into()),
                    context_files: None,
                },
                RepoEntry {
                    id: "docs".into(),
                    path: "./docs".into(),
                    role: Some("docs".into()),
                    tags: vec!["docs".into()],
                    depends_on: vec![],
                    description: Some("Documentation site".into()),
                    context_files: None,
                },
            ],
        }
    }

    #[test]
    fn synonym_login_hits_oauth_repos() {
        let w = ws();
        let ranked = rank_repos(&w, "login");
        let ids: Vec<_> = ranked.iter().map(|r| r.repo.id.as_str()).collect();
        assert!(ids.contains(&"core") || ids.contains(&"app"));
        assert!(!ids.contains(&"docs") || ranked[0].repo.id != "docs");
    }

    #[test]
    fn multi_token_prefers_coverage() {
        let w = ws();
        let ranked = rank_repos(&w, "identity payments");
        assert!(!ranked.is_empty());
        assert_eq!(ranked[0].repo.id, "core");
        assert!(ranked[0].token_hits >= 2);
    }

    #[test]
    fn expand_oauth() {
        let v = expand_token("oauth");
        assert!(v.iter().any(|x| x == "auth"));
    }
}
