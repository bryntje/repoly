//! Query ranking for multi-repo selection (`ctx` / `plan`).

use crate::config::{RepoEntry, Workspace, DEFAULT_CONTEXT_FILES};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Max bytes of each context file used for content matching.
const SNIPPET_BYTES: usize = 12_000;

/// Light stopwords stripped when other tokens remain (ranking only).
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "in", "on", "for", "to", "with", "and", "or", "please", "help", "me",
    "my", "our", "this", "that", "from", "into", "about", "across",
];

/// Built-in multi-word phrase → extra tokens (before synonym expand).
fn builtin_rewrites() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("growth checkin", &["reflections", "journal", "growth"]),
        ("growth check-in", &["reflections", "journal", "growth"]),
        ("login flow", &["oauth", "auth", "identity", "login"]),
        ("sign in", &["oauth", "auth", "login"]),
        ("sign-in", &["oauth", "auth", "login"]),
        ("payment flow", &["payments", "billing", "checkout"]),
        ("pay wall", &["payments", "premium", "billing"]),
        ("paywall", &["payments", "premium", "billing"]),
    ]
}

/// Result of normalizing a free-text query for ranking.
#[derive(Debug, Clone)]
pub struct NormalizedQuery {
    pub original: String,
    pub tokens: Vec<String>,
    pub rewrites_applied: Vec<String>,
}

/// Normalize a query: lowercase, apply phrase rewrites, tokenize, drop stopwords
/// when other content remains.
pub fn normalize_query(workspace: &Workspace, query: &str) -> NormalizedQuery {
    let original = query.trim().to_string();
    let q = original.to_lowercase();
    let mut rewrites_applied = Vec::new();
    let mut injected: Vec<String> = Vec::new();

    // Workspace rewrites first (more specific), then built-ins.
    for rw in &workspace.ranking.rewrites {
        let phrase = rw.match_phrase.trim().to_lowercase();
        if phrase.len() < 2 {
            continue;
        }
        if q.contains(&phrase) {
            rewrites_applied.push(format!("{phrase}→{}", rw.add.join(",")));
            for t in &rw.add {
                let t = t.trim().to_lowercase();
                if t.len() >= 2 {
                    push_unique(&mut injected, &t);
                }
            }
        }
    }
    for (phrase, add) in builtin_rewrites() {
        if q.contains(phrase) {
            rewrites_applied.push(format!("{phrase}→{}", add.join(",")));
            for t in *add {
                push_unique(&mut injected, t);
            }
        }
    }

    // Tokenize original query (after lowercasing).
    let mut tokens: Vec<String> = q
        .split(|c: char| c.is_whitespace() || c == ',' || c == '/' || c == '|')
        .map(str::trim)
        .filter(|t| t.len() >= 2)
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_'))
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect();

    for t in injected {
        push_unique(&mut tokens, &t);
    }

    // Drop stopwords only if something substantive remains.
    let filtered: Vec<String> = tokens
        .iter()
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .cloned()
        .collect();
    if !filtered.is_empty() {
        tokens = filtered;
    }

    NormalizedQuery {
        original,
        tokens,
        rewrites_applied,
    }
}

/// Expand a query token with built-in synonyms (English polyrepo/product terms).
pub fn expand_token_builtin(token: &str) -> Vec<String> {
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
        "payment" | "payments" | "billing" | "checkout" | "mollie" | "premium" => &[
            "payments", "payment", "checkout", "mollie", "premium", "billing",
        ],
        "discord" | "bot" | "guild" => &["discord", "bot", "guild", "alphapy"],
        "agent" | "agents" | "hermit" | "hermes" => {
            &["agents", "agent", "hermit", "memory", "vault"]
        }
        "reflection" | "reflections" | "journal" | "growth" | "checkin" | "growthcheckin"
        | "growth-checkin" => &[
            "reflections",
            "reflection",
            "journal",
            "encryption",
            "growth",
            "checkin",
            "growthcheckin",
        ],
        // Only helps if tags/desc mention these; still empty for pure "fix the bug"
        "bug" | "bugs" | "fix" | "hotfix" | "issue" | "issues" => {
            &["bug", "bugs", "fix", "hotfix", "issue", "issues"]
        }
        "perf" | "performance" | "latency" | "slow" => &["performance", "perf", "latency", "slow"],
        "telemetry" | "metrics" | "ops" | "cockpit" => {
            &["telemetry", "metrics", "ops", "mind", "railway"]
        }
        "ui" | "style" | "styling" | "theme" | "design" => &["ui", "styling", "theme", "frontend"],
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
        push_unique(&mut out, e);
    }
    out
}

/// Built-in expand (backward-compatible name).
pub fn expand_token(token: &str) -> Vec<String> {
    expand_token_builtin(token)
}

/// Expand a token using built-ins plus workspace `[ranking].synonym_groups`.
pub fn expand_token_for(workspace: &Workspace, token: &str) -> Vec<String> {
    let mut out = expand_token_builtin(token);
    let t = token.to_lowercase();
    for group in &workspace.ranking.synonym_groups {
        let members: Vec<String> = group
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if members.iter().any(|m| m == &t) {
            for m in members {
                push_unique(&mut out, &m);
            }
        }
    }
    out
}

fn push_unique(out: &mut Vec<String>, s: &str) {
    if !out.iter().any(|x| x == s) {
        out.push(s.to_string());
    }
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
    let normalized = normalize_query(workspace, query);
    if normalized.tokens.is_empty() {
        return Vec::new();
    }

    let original_tokens = &normalized.tokens;

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
            let mut token_structured = 0i32;
            let mut token_file = 0i32;
            let mut token_reasons: Vec<String> = Vec::new();

            let variants = expand_token_for(workspace, token);
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
                } else if tags
                    .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
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
    for doc in &workspace.context.always {
        let rel = doc.path();
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
        if crate::policy::should_skip_context_path(
            f,
            f,
            &workspace.policy.skip_globs,
            workspace.policy.use_builtin_secret_filters,
        ) {
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

    #[test]
    fn expand_growth_in_reflection_cluster() {
        let v = expand_token("growth");
        assert!(v.iter().any(|x| x == "reflections"));
    }

    #[test]
    fn workspace_synonym_groups_merge() {
        let mut w = ws();
        w.ranking.synonym_groups = vec![vec!["billing".into(), "invoice".into(), "stripe".into()]];
        // tag "payments" on core won't hit; add stripe tag via group → need a repo with stripe tag
        w.repos[0].tags.push("stripe".into());
        let ranked = rank_repos(&w, "invoice");
        assert!(
            ranked.iter().any(|r| r.repo.id == "core"),
            "expected core via config synonym invoice→stripe tag, got {:?}",
            ranked.iter().map(|r| &r.repo.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn hermit_style_reflection_tag() {
        let mut w = ws();
        w.repos.push(RepoEntry {
            id: "hermit".into(),
            path: "./hermit".into(),
            role: Some("agent".into()),
            tags: vec!["hermit".into(), "reflections".into(), "growth".into()],
            depends_on: vec!["core".into()],
            description: Some("Agent host".into()),
            context_files: None,
        });
        let ranked = rank_repos(&w, "reflection related");
        let ids: Vec<_> = ranked.iter().map(|r| r.repo.id.as_str()).collect();
        assert!(ids.contains(&"app"), "{ids:?}");
        assert!(ids.contains(&"hermit"), "{ids:?}");
    }

    #[test]
    fn normalize_strips_stopwords() {
        let w = ws();
        let n = normalize_query(&w, "please fix the oauth login");
        assert!(n.tokens.iter().any(|t| t == "oauth"));
        assert!(n.tokens.iter().any(|t| t == "login") || n.tokens.iter().any(|t| t == "fix"));
        assert!(!n.tokens.iter().any(|t| t == "please" || t == "the"));
    }

    #[test]
    fn builtin_phrase_rewrite_login_flow() {
        let w = ws();
        let n = normalize_query(&w, "login flow broken");
        assert!(
            n.tokens.iter().any(|t| t == "oauth" || t == "identity"),
            "expected oauth/identity injected, got {:?}",
            n.tokens
        );
        assert!(!n.rewrites_applied.is_empty());
    }

    #[test]
    fn workspace_rewrite_injects_tokens() {
        use crate::config::RankingRewrite;
        let mut w = ws();
        w.ranking.rewrites = vec![RankingRewrite {
            match_phrase: "invoice portal".into(),
            add: vec!["stripe".into(), "billing".into()],
        }];
        w.repos[0].tags.push("stripe".into());
        let ranked = rank_repos(&w, "open the invoice portal please");
        assert!(
            ranked.iter().any(|r| r.repo.id == "core"),
            "expected core via rewrite→stripe, got {:?}",
            ranked.iter().map(|r| &r.repo.id).collect::<Vec<_>>()
        );
    }
}
