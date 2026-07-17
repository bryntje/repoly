//! Library-level integration tests (no process spawn).

mod common;

use common::WorkspaceFixture;
use repoly::commit::{self, CommitOpts};
use repoly::config::load_config;
use repoly::context;
use repoly::plan;
use repoly::rank;
use repoly::run::{self, LaunchMode};
use repoly::status;

#[test]
fn load_fixture_config() {
    let fx = WorkspaceFixture::basic();
    let ws = load_config(&fx.config()).unwrap();
    assert_eq!(ws.name, "fixture");
    assert_eq!(ws.repos.len(), 2);
}

#[test]
fn status_detects_dirty() {
    let fx = WorkspaceFixture::basic();
    let ws = load_config(&fx.config()).unwrap();
    let report = status::collect_status(&ws, None, false);
    assert!(report.repos.iter().all(|r| !r.dirty));

    fx.dirty_web();
    let report = status::collect_status(&ws, Some(&["web".into()]), false);
    assert_eq!(report.repos.len(), 1);
    assert!(report.repos[0].dirty);
    assert!(report.repos[0].dirty_count >= 1);
}

#[test]
fn plan_with_deps_includes_api() {
    let fx = WorkspaceFixture::basic();
    let ws = load_config(&fx.config()).unwrap();
    let p = plan::build_plan(&ws, Some("login"), None, None, None, true, true).unwrap();
    let ids: Vec<_> = p.steps.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"web"));
    assert!(
        ids.contains(&"api"),
        "api should be pulled as depends_on: {ids:?}"
    );
}

#[test]
fn rank_payments_prefers_api() {
    let fx = WorkspaceFixture::basic();
    let ws = load_config(&fx.config()).unwrap();
    let ranked = rank::rank_repos(&ws, "payments");
    assert!(!ranked.is_empty());
    assert_eq!(ranked[0].repo.id, "api");
}

#[test]
fn context_pack_has_always_doc() {
    let fx = WorkspaceFixture::basic();
    let ws = load_config(&fx.config()).unwrap();
    let pack = context::build_context(
        &ws,
        Some("oauth"),
        None,
        None,
        None,
        Some(50_000),
        true,
        false,
    )
    .unwrap();
    assert!(!pack.selected_repos.is_empty());
    let md = context::format_markdown(&pack);
    assert!(md.contains("Shared always-doc") || md.contains("PLATFORM"));
}

#[test]
fn exec_capture_argv() {
    let fx = WorkspaceFixture::basic();
    let ws = load_config(&fx.config()).unwrap();
    let repo = run::resolve_repo(&ws, "api").unwrap();
    let r = run::exec_capture(
        &ws,
        repo,
        &[
            "git".into(),
            "rev-parse".into(),
            "--abbrev-ref".into(),
            "HEAD".into(),
        ],
        LaunchMode::Argv,
    )
    .unwrap();
    assert!(r.success());
    assert!(r.stdout.as_deref().unwrap_or("").contains("main"));
}

#[test]
fn commit_all_creates_sha() {
    let fx = WorkspaceFixture::basic();
    fx.dirty_web();
    let ws = load_config(&fx.config()).unwrap();
    let repo = run::resolve_repo(&ws, "web").unwrap();
    let r = commit::commit_one(
        &ws,
        repo,
        &CommitOpts {
            message: "chore: lib integration".into(),
            all: true,
            paths: vec![],
            amend: false,
            allow_empty: false,
            no_verify: false,
            dry_run: false,
            signoff: false,
        },
    )
    .unwrap();
    assert!(r.success, "{r:?}");
    assert!(r.commit_sha.is_some());
}
