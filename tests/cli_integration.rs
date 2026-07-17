//! End-to-end CLI tests against a temporary multi-repo workspace.

mod common;

use assert_cmd::cargo::cargo_bin_cmd;
use common::WorkspaceFixture;
use predicates::prelude::*;
use std::process::Command;

fn poly() -> assert_cmd::Command {
    cargo_bin_cmd!("poly")
}

#[test]
fn version_prints() {
    poly()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("poly"));
}

#[test]
fn list_and_validate_fixture() {
    let fx = WorkspaceFixture::basic();
    poly()
        .args(["validate", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains("2 repos"));

    poly()
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
    let out = poly()
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
    let out = poly()
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
    let out = poly()
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
    let out = poly()
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
}

#[test]
fn ctx_includes_always_doc_and_selection() {
    let fx = WorkspaceFixture::basic();
    poly()
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
fn path_and_root() {
    let fx = WorkspaceFixture::basic();
    poly()
        .args(["root", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            fx.path().file_name().unwrap().to_str().unwrap(),
        ));

    let out = poly()
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
    poly()
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
    poly()
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
    poly()
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
    poly()
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
    poly()
        .args(["commit", "web", "-m", "should skip", "--config"])
        .arg(fx.config())
        .assert()
        .success()
        .stderr(predicate::str::contains("[skip]"));

    fx.dirty_web();
    poly()
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
    poly()
        .current_dir(tmp.path())
        .arg("list")
        .env_remove("POLY_CONFIG")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("workspace not found"));
}

#[test]
fn init_writes_poly_toml() {
    let tmp = tempfile::TempDir::new().unwrap();
    poly()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));
    assert!(tmp.path().join("poly.toml").is_file());
}
