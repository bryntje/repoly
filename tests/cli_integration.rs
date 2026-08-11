//! End-to-end CLI tests against a temporary multi-repo workspace.

mod common;

use assert_cmd::cargo::cargo_bin_cmd;
use common::WorkspaceFixture;
use predicates::prelude::*;
use std::process::Command;

fn repoly_cmd() -> assert_cmd::Command {
    cargo_bin_cmd!("repoly")
}

#[test]
fn version_prints() {
    repoly_cmd()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("repoly"));
}

#[test]
fn list_and_validate_fixture() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args(["validate", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("2 repos"));

    repoly_cmd()
        .args(["list", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("api"))
        .stdout(predicate::str::contains("web"));
}

#[test]
fn list_json_shape() {
    let fx = WorkspaceFixture::basic();
    let out = repoly_cmd()
        .args(["list", "--format", "json", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["workspace"], "fixture");
    assert_eq!(v["repos"].as_array().unwrap().len(), 2);
}

#[test]
fn status_json_clean_and_dirty() {
    let fx = WorkspaceFixture::basic();
    let out = repoly_cmd()
        .args(["status", "--format", "json", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["workspace"], "fixture");
    let repos = v["repos"].as_array().unwrap();
    assert!(repos.iter().all(|r| r["dirty"] == false));

    fx.dirty_web();
    let out = repoly_cmd()
        .args(["status", "--format", "json", "--repos", "web", "--config"])
        .arg(fx.config())
        .assert()
        .success() // dirty alone is still exit 0
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["repos"][0]["id"], "web");
    assert_eq!(v["repos"][0]["dirty"], true);
}

#[test]
fn plan_orders_api_before_web() {
    let fx = WorkspaceFixture::basic();
    let out = repoly_cmd()
        .args([
            "plan",
            "oauth",
            "--format",
            "json",
            "--no-status",
            "--config",
        ])
        .arg(fx.config())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let steps = v["steps"].as_array().unwrap();
    assert!(steps.len() >= 2);
    let ids: Vec<&str> = steps.iter().map(|s| s["id"].as_str().unwrap()).collect();
    let i_api = ids.iter().position(|x| *x == "api").expect("api");
    let i_web = ids.iter().position(|x| *x == "web").expect("web");
    assert!(i_api < i_web, "api should come before web: {ids:?}");
    // --no-status omits summary and structured status
    assert!(v["status_summary"].is_null());
    assert!(steps[0]["status"].is_null());
}

#[test]
fn plan_status_summary_and_structured_fields() {
    let fx = WorkspaceFixture::basic();
    fx.dirty_web();
    let out = repoly_cmd()
        .args(["plan", "oauth", "--format", "json", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(v["status_summary"]["dirty"].as_u64().unwrap() >= 1);
    let steps = v["steps"].as_array().unwrap();
    let web = steps.iter().find(|s| s["id"] == "web").expect("web step");
    assert_eq!(web["status"]["dirty"], true);
    assert!(web["status_line"].as_str().unwrap().contains("dirty"));
}

#[test]
fn plan_and_ctx_format_grok() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args([
            "plan",
            "oauth",
            "--format",
            "grok",
            "--no-status",
            "--config",
        ])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("repoly plan for Grok"))
        .stdout(predicate::str::contains("## Instructions"))
        .stdout(predicate::str::contains("## Next"));

    repoly_cmd()
        .args([
            "ctx",
            "oauth",
            "--format",
            "grok",
            "--no-status",
            "--config",
        ])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("repoly context for Grok"))
        .stdout(predicate::str::contains("## Instructions"))
        .stdout(predicate::str::contains("Shared always-doc"));
}

#[test]
fn ctx_includes_always_doc_and_selection() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args([
            "ctx",
            "oauth",
            "--format",
            "prompt",
            "--no-status",
            "--config",
        ])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("Shared always-doc"))
        .stdout(predicate::str::contains("Selected:"));
}

#[test]
fn doctor_suggests_untracked_git_dir() {
    let fx = WorkspaceFixture::basic();
    // Extra sibling not in repoly.toml
    let extra = fx.path().join("orphan-svc");
    std::fs::create_dir_all(&extra).unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(&extra)
        .status()
        .unwrap();

    repoly_cmd()
        .args(["doctor", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("orphan-svc"))
        .stdout(predicate::str::contains("not in repoly.toml"));
}

#[test]
fn path_and_root() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args(["root", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            fx.path().file_name().unwrap().to_str().unwrap(),
        ));

    let out = repoly_cmd()
        .args(["path", "web", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let p = String::from_utf8_lossy(&out);
    assert!(p.contains("web"));
}

#[test]
fn exec_runs_in_repo_cwd() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args(["exec", "api", "--config"])
        .arg(fx.config())
        .args(["--", "git", "rev-parse", "--abbrev-ref", "HEAD"])
        .assert()
        .success()
        .stdout(predicate::str::contains("main"));
}

#[test]
fn exec_shell_pipe() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args(["exec", "api", "--shell", "--config"])
        .arg(fx.config())
        .args(["--", "printf 'hi' | tr h H"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hi"));
}

#[test]
fn run_multi_repo() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args(["run", "--repos", "api,web", "--config"])
        .arg(fx.config())
        .args(["--", "git", "rev-parse", "--is-inside-work-tree"])
        .assert()
        .success()
        .stderr(predicate::str::contains("[ok]"));
}

#[test]
fn run_refuses_all_repos() {
    let fx = WorkspaceFixture::basic();
    repoly_cmd()
        .args(["run", "--config"])
        .arg(fx.config())
        .args(["--", "true"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --repos"));
}

#[test]
fn commit_dry_run_and_real() {
    let fx = WorkspaceFixture::basic();
    // nothing staged → skip
    repoly_cmd()
        .args(["commit", "web", "-m", "should skip", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stderr(predicate::str::contains("[skip]"));

    fx.dirty_web();
    repoly_cmd()
        .args([
            "commit",
            "web",
            "-m",
            "test: fixture commit",
            "--all",
            "--config",
        ])
        .arg(fx.config())
        .assert()
        .success()
        .stderr(predicate::str::contains("[ok]"));

    // verify git log
    let log = Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(fx.path().join("web"))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&log.stdout).contains("test: fixture commit"));
}

#[test]
fn missing_workspace_exit_code() {
    let tmp = tempfile::TempDir::new().unwrap();
    repoly_cmd()
        .current_dir(tmp.path())
        .arg("list")
        .env_remove("REPOLY_CONFIG")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("workspace not found"));
}

#[test]
fn init_writes_repoly_toml() {
    let tmp = tempfile::TempDir::new().unwrap();
    repoly_cmd()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));
    assert!(tmp.path().join("repoly.toml").is_file());
}
