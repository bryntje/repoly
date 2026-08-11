# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Human CLI formatting** — hierarchical headers/meta, severity badges, and optional color for `doctor`, `list`, `status`, `validate`, `run`/`commit` summaries
  - Global `--color auto|always|never` (default auto; respects `NO_COLOR` / `CLICOLOR_FORCE`)
  - `REPOLY_ASCII=1` forces ASCII badges instead of unicode symbols
  - Agent formats (`json` / `prompt` / `grok` / MCP) stay plain (no ANSI)

## [0.3.1] — 2026-08-11

### Fixed

- **`repoly doctor` ctx smoke** uses the workspace `max_chars` (no silent 64k cap) so budget tips match real `ctx` / `build_context`

## [0.3.0] — 2026-08-11

### Added

- **Smart Always-Docs (B+C)** — richer always-doc packing: optional tags, section extraction, priority ordering for work-mode packs
- **`ctx` / `plan` `--format grok`** (CLI + MCP `format=grok`) — Grok-oriented briefs with Instructions + Next
- **Plan status polish** — `status_summary` banner; structured `status` on each step (JSON); dirty/missing hints in suggested commands
- **Query normalize / rewrite** — stopword strip, built-in phrase rewrites, optional `[[ranking.rewrites]]` (`match` + `add`); `query_normalized` on plan JSON
- **`repoly doctor` untracked-repo suggestions** — shallow scan of workspace root for sibling git dirs not in `repoly.toml` (`info` only, with suggested `[[repos]]` line)
- **`init --from-code-workspace` role + tags inference** — heuristic `role` / `tags` from folder names and paths

### Changed

- Plan default suggested ctx command uses `--format grok`
- Docs/examples/skill prefer `format=grok` in the agent workflow

### Fixed

- Innersync example context budget settings (realistic `max_chars` / reserve / always caps)
- Naming-schema propagation matching for the meta repo in the Innersync example

## [0.2.2] — 2026-07-17

### Added

- **Context packing v2** — work-mode packs reserve room for selected-repo files (default 40% of `max_chars`)
  - `context.repo_reserve_pct` and `context.always_max_chars` in `repoly.toml`
  - Pack `budget` stats + tips in JSON/prompt/markdown (`always X/cap`, `repo_files`, truncation tips)
- **`repoly doctor`** — workspace health: paths, always-doc vs budget, ranking/policy, MCP/ctx tips, ctx smoke

### Changed

- Work-mode `ctx` / MCP `build_context` no longer lets always-docs consume the entire budget before repo AGENTS/README

## [0.2.1] — 2026-07-17

### Added

- **`[ranking].synonym_groups`** in `repoly.toml` — workspace-specific term clusters for plan/ctx (merged with built-ins)
- Built-in synonym clusters expanded: growth/checkin ↔ reflections; bug/fix/issue; perf/latency
- Docs/examples note optional `[ranking]` section

## [0.2.0] — 2026-07-17

Security **layer 2**: path confinement, command policy, resource limits, richer context skips, optional audit. Generic for any polyrepo — no product-specific defaults.

### Added

- **`src/policy.rs`** — shared path confinement, exec bin policy, skip globs, audit helper
- **Commit path confinement** — pathspecs must resolve under the target repo root (always on)
- **MCP default bin deny** — with `--allow-exec`, sensitive basenames blocked (`sudo`, `dd`, `mkfs*`, `shutdown`, …)
- **`--exec-bin-allow` / `--exec-bin-deny` / `--no-default-exec-deny`**
- **Shell vs bin policy** — `shell=true` rejected while any bin policy (including default deny) is active
- **`--exec-timeout-secs` / `--exec-max-output-bytes`** — kill hung MCP children; cap stdout/stderr (JSON: `timed_out`, `*_truncated`)
- **`--audit-log PATH`** — JSONL events for `exec` / `run` / `commit`
- **`[policy]` in `repoly.toml`** (optional): `skip_globs`, `use_builtin_secret_filters`, `exec_timeout_secs`, `exec_max_output_bytes`, `audit_log`
- Session flags override workspace `[policy]` when both are set
- **SECURITY.md** — threat model, layer 1 vs 2, residual risk, recommended host profile
- Tests for deny/allow, shell block, timeout, truncation, audit, path escape, skip globs

### Changed

- MCP `run` uses capture limits path (sequential with session opts)
- `docs/mcp.md` / README — hardened agent examples; shell requires inactive bin policy

### Notes

- Default bin deny on MCP `--allow-exec` is a **behavior change** from 0.1.x (stricter). Opt out with `--no-default-exec-deny`.
- Plain CLI `repoly exec` remains unrestricted (human threat model).
- Recommended agent host: `--allow-exec` + `--exec-repos` + `--exec-bin-allow` + timeout/max-output + optional audit.

## [0.1.1] — 2026-07-17

### Fixed

- Windows release packaging (PowerShell `Compress-Archive` instead of missing `zip`)
- Intel macOS builds no longer wait on scarce `macos-13` runners (cross-target on `macos-latest`)
- CI: drop hung Windows test job; cancel superseded workflow runs
- crates.io keywords include `repoly`

### Changed

- GitHub Actions bumped toward Node 24-friendly versions (`checkout`/`upload-artifact`/`download-artifact`)

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

[Unreleased]: https://github.com/bryntje/repoly/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/bryntje/repoly/releases/tag/v0.3.1
[0.3.0]: https://github.com/bryntje/repoly/releases/tag/v0.3.0
[0.2.2]: https://github.com/bryntje/repoly/releases/tag/v0.2.2
[0.2.1]: https://github.com/bryntje/repoly/releases/tag/v0.2.1
[0.2.0]: https://github.com/bryntje/repoly/releases/tag/v0.2.0
[0.1.1]: https://github.com/bryntje/repoly/releases/tag/v0.1.1
[0.1.0]: https://github.com/bryntje/repoly/releases/tag/v0.1.0
