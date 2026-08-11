//! Smart always-doc handling: section extraction + tag/query relevance.

use std::collections::HashSet;

/// A single section extracted from an always-doc.
#[derive(Debug, Clone)]
pub struct AlwaysSection {
    pub title: String,
    pub content: String,
    pub score: i32,
}

/// Split a markdown document into sections based on `## ` and `### ` headers.
pub fn extract_sections(raw: &str) -> Vec<AlwaysSection> {
    let mut sections = Vec::new();
    let mut current_title = String::from("Top");
    let mut current_content = String::new();

    for line in raw.lines() {
        if line.starts_with("## ") || line.starts_with("### ") {
            if !current_content.trim().is_empty() {
                sections.push(AlwaysSection {
                    title: current_title.clone(),
                    content: current_content.trim().to_string(),
                    score: 0,
                });
            }
            current_title = line.trim_start_matches('#').trim().to_string();
            current_content.clear();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    if !current_content.trim().is_empty() {
        sections.push(AlwaysSection {
            title: current_title,
            content: current_content.trim().to_string(),
            score: 0,
        });
    }

    sections
}

/// Score sections based on query + repo tags overlap.
pub fn score_sections(
    sections: &mut [AlwaysSection],
    query: Option<&str>,
    repo_tags: &[String],
    doc_tags: &[String],
    preferred_sections: &[String],
) {
    let query_lower = query.unwrap_or("").to_lowercase();
    let query_tokens: HashSet<&str> = query_lower.split_whitespace().collect();

    let all_tags: HashSet<&str> = repo_tags
        .iter()
        .chain(doc_tags.iter())
        .map(|s| s.as_str())
        .collect();

    for section in sections.iter_mut() {
        let title_lower = section.title.to_lowercase();
        let content_lower = section.content.to_lowercase();

        let mut score = 0;

        // Tag matches
        for tag in &all_tags {
            if title_lower.contains(tag) || content_lower.contains(tag) {
                score += 15;
            }
        }

        // Query token matches
        for token in &query_tokens {
            if title_lower.contains(token) || content_lower.contains(token) {
                score += 10;
            }
        }

        // Bonus for explicit section preference
        if !doc_tags.is_empty() {
            score += 5;
        }
        for pref in preferred_sections {
            if title_lower.contains(&pref.to_lowercase()) {
                score += 25;
            }
        }

        section.score = score;
    }

    // Sort by score descending
    sections.sort_by_key(|b| std::cmp::Reverse(b.score));
}

/// Filter and return the most relevant sections up to a byte budget.
pub fn select_relevant_sections(
    sections: Vec<AlwaysSection>,
    max_bytes: usize,
) -> (Vec<AlwaysSection>, bool) {
    let mut selected = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;

    for sec in sections {
        let size = sec.content.len();
        if used + size > max_bytes {
            truncated = true;
            break;
        }
        used += size;
        selected.push(sec);
    }

    (selected, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_sections() {
        let md = "## Naming\nCore rules here\n\n## Agents\nAgent rules";
        let secs = extract_sections(md);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].title, "Naming");
        assert_eq!(secs[1].title, "Agents");
    }

    #[test]
    fn scores_sections() {
        let mut secs = vec![
            AlwaysSection {
                title: "Naming".into(),
                content: "naming schema".into(),
                score: 0,
            },
            AlwaysSection {
                title: "Agents".into(),
                content: "hermit".into(),
                score: 0,
            },
        ];
        score_sections(&mut secs, Some("naming"), &[], &["naming".into()], &[]);
        assert!(secs[0].score > secs[1].score);
    }
}
