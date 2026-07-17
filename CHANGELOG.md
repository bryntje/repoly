# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- GitHub Actions **Release** workflow: multi-platform binaries + checksums on `v*` tags

## [0.1.0] — 2026-07-17

First **public** release of `repoly` (internal dogfood history through 0.10.x is not versioned for crates.io).

### Added

- **Workspace model** — `repoly.toml` (`schema_version = 1`) with repos, roles, tags, `depends_on`, and context docs
- **Config discovery** — `--config`, `REPOLY_CONFIG`, walk-up `repoly.toml` / `.repoly/repoly.toml` (legacy `poly.toml` still found)
- **`repoly init`** — skeleton config; optional import from VS Code / Cursor `.code-workspace`
- **`repoly validate` / `list` / `root` / `path`**
- **`repoly status`** — parallel cross-repo git status (branch, dirty, ahead/behind); table + JSON
- **`repoly plan`** — keyword/tag selection + topological `depends_on` order; markdown / prompt / JSON
- **`repoly ctx`** — agent-ready context packs (always-docs + selected repo AGENTS/README); markdown / prompt / JSON; optional `--with-deps`
- **Smarter ranking** — synonyms, multi-token coverage, structured tags/id preferred over file noise
- **`repoly exec` / `run`** — run commands in one or many repo cwds; optional `--shell` (`sh -c` / `cmd /C`)
- **`repoly commit`** — safe per-repo (or filtered multi-repo) git commit with `--all` / pathspecs
- **MCP stdio server** (`repoly mcp`) — tools: `list_repos`, `status`, `plan`, `build_context`, `repo_path`, `workspace_root`, and opt-in `exec` / `run` / `commit`
- **MCP safety** — mutation requires `--allow-exec`; shell requires `--allow-shell`; optional `--exec-repos` allowlist
- **Child env** — `REPOLY_WORKSPACE`, `REPOLY_ROOT`, `REPOLY_REPO`, `REPOLY_REPO_PATH`, `REPOLY_REPO_ROLE`
- **Tests** — unit + CLI e2e + library e2e + MCP protocol tests
- **CI** — GitHub Actions (fmt, clippy, test on Ubuntu + macOS)
- **License** — MIT OR Apache-2.0
- **Docs** — README, CONTRIBUTING, SECURITY, examples

### Notes

- Windows support is **experimental** (CI does not hard-fail Windows yet).
- Binary distribution: GitHub Releases on `v*` tags (macOS arm/x64, Linux x64, Windows x64).
- crates.io: `cargo install repoly` after publish.

[Unreleased]: https://github.com/bryntje/repoly/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/bryntje/repoly/releases/tag/v0.1.0
