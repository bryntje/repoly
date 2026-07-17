# repoly

**Multi-repo workspace awareness for the terminal.**

[![CI](https://github.com/bryntje/repoly/actions/workflows/ci.yml/badge.svg)](https://github.com/bryntje/repoly/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)
[![Version](https://img.shields.io/badge/version-0.1.1-informational.svg)](./CHANGELOG.md)
[![Crates.io](https://img.shields.io/crates/v/repoly.svg)](https://crates.io/crates/repoly)

Local CLI for **polyrepo** product stacks. Gives humans and AI coding tools (Grok, Claude Code, Codex, …) what IDE multi-root workspaces already have: a machine-readable map of repos, cross-repo status, and **agent-ready context packs**.

No cloud. No IDE required. No forks of your coding agent.

→ [Changelog](./CHANGELOG.md) · [MCP setup](./docs/mcp.md) · [Contributing](./CONTRIBUTING.md)

---

## Install

Requires **`git`** on `PATH`.

### From source

```bash
cargo install --path .
# or
cargo build --release   # → target/release/repoly
```

### From GitHub Releases (recommended once tagged)

1. Open [Releases](https://github.com/bryntje/repoly/releases)
2. Download the archive for your OS/CPU (see names below)
3. Extract and put `repoly` on your `PATH`
4. Verify the matching `.sha256` file

| Archive | Platform |
|---------|----------|
| `repoly-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `repoly-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `repoly-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 |
| `repoly-x86_64-pc-windows-msvc.zip` | Windows x86_64 (experimental) |

Releases are produced automatically when a `v*` tag is pushed (see [CONTRIBUTING.md](./CONTRIBUTING.md#releasing-maintainers)).

### From crates.io (after publish)

```bash
cargo install repoly
```

Check:

```bash
repoly version   # → repoly 0.1.1
```

```bash
# crates.io (primary for most users)
cargo install repoly
```

**Platforms:** macOS and Linux are first-class (CI on every push). Windows binaries may be built on release tags, but Windows is **experimental** (not in PR CI — it was hanging on full `cargo test`).

---

## 30-second demo

Use the bundled minimal workspace:

```bash
cd examples/minimal
repoly validate
repoly list
repoly plan "oauth"
repoly ctx --format prompt "payments" | head
```

Typical flow on your own stack:

```bash
cd /path/to/checkout-parent   # folder that contains all product repos
repoly init                  # or: repoly init --from-code-workspace App.code-workspace
# edit repoly.toml → point paths at your repos, add tags / depends_on

repoly status
repoly plan "login checkout"
repoly ctx --format prompt "login checkout" > /tmp/brief.md
```

Then open a coding agent **in one repo**:

```bash
repoly exec web -- grok "Read /tmp/brief.md; only change this repo unless asked."
# or
cd "$(repoly path web)" && claude
```

---

## Why repoly?

| Approach | Gap |
|----------|-----|
| VS Code / Cursor multi-root | Great for humans; not a CLI/agent control plane |
| myrepos / meta / gita | Batch git; no agent context packs |
| Nx / Turborepo | Monorepo task graphs; wrong shape for many product polyrepos |
| Parallel agent worktrees | Usually one repo; not cross-repo ownership |

**repoly** = declarative polyrepo workspace + status + plan + context + optional scoped exec.

---

## Commands

| Command | Purpose |
|---------|---------|
| `repoly init` | Create `repoly.toml` (optional `--from-code-workspace`) |
| `repoly validate` | Schema + path checks (`--strict`) |
| `repoly list` | List repos (`--format json`) |
| `repoly status` | Branch / dirty / ahead / behind |
| `repoly plan [query]` | Which repos + `depends_on` order |
| `repoly ctx [query]` | Context pack for humans/agents |
| `repoly root` / `path <id>` | Scripting helpers |
| `repoly exec <repo> -- <cmd…>` | Run in one repo cwd |
| `repoly run --repos a,b -- <cmd…>` | Same command across repos |
| `repoly commit …` | Safe git commit in workspace repo(s) |
| `repoly mcp` | MCP stdio server |

### Common patterns

```bash
# Ranking + order
repoly plan "identity oauth"
repoly ctx --format prompt "identity oauth"
repoly ctx --with-deps --repos web --format markdown

# Execution
repoly exec api -- npm test
repoly run --repos api,web -- git status -sb
repoly exec web --shell -- 'npm test && echo ok'   # shell is opt-in

# Commits (only mapped repos)
repoly commit web -m "fix: login redirect" --all
repoly commit --repos api,web -m "chore: related fixes" --all --dry-run
```

**Exit codes:** `0` ok · `1` error · `2` partial · `3` no workspace.

Child env: `REPOLY_WORKSPACE`, `REPOLY_ROOT`, `REPOLY_REPO`, `REPOLY_REPO_PATH`, `REPOLY_REPO_ROLE`.

---

## MCP (Grok / Claude / Cursor)

Full guide: **[docs/mcp.md](./docs/mcp.md)**.

**Read-only** (recommended default) — Grok `~/.grok/config.toml`:

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp"]
env = { REPOLY_CONFIG = "/absolute/path/to/repoly.toml" }
```

**With mutation** (prefer repo allowlist; default sensitive-bin deny is on — see [SECURITY.md](SECURITY.md)):

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp", "--allow-exec", "--exec-repos", "api,web"]
env = { REPOLY_CONFIG = "/absolute/path/to/repoly.toml" }
```

**Hardened agent host** (optional bin allowlist + limits):

```toml
args = [
  "mcp",
  "--allow-exec",
  "--exec-repos", "api,web",
  "--exec-bin-allow", "git,cargo,npm,node,python,python3,go,make",
  "--exec-timeout-secs", "120",
  "--exec-max-output-bytes", "262144",
]
```

```bash
grok mcp add repoly -- repoly mcp
# or with exec:
grok mcp add repoly -- repoly mcp --allow-exec --exec-repos api,web
```

| Tool | Notes |
|------|--------|
| `list_repos`, `status`, `plan`, `build_context`, `repo_path`, `workspace_root` | Always available |
| `exec`, `run`, `commit` | Need `--allow-exec`; default bin deny; optional `--exec-bin-allow` / `--exec-bin-deny` |
| shell mode | Needs `--allow-shell` **and** inactive bin policy (`--no-default-exec-deny`, no custom lists) |

---

## Config (`repoly.toml`)

```toml
schema_version = 1

[workspace]
name = "acme"

[context]
always = ["docs/PLATFORM.md"]
max_chars = 48000

[[repos]]
id = "api"
path = "./api"
role = "api"
tags = ["backend", "payments"]
description = "HTTP API"

[[repos]]
id = "web"
path = "./web"
role = "frontend"
tags = ["frontend", "oauth"]
depends_on = ["api"]
description = "Web app"
```

**Discovery:** `--config` → `$REPOLY_CONFIG` → walk-up `repoly.toml` or `.repoly/repoly.toml` (legacy `poly.toml` still accepted).

Examples:

- [`examples/minimal/`](./examples/minimal/) — api + web demo  
- [`examples/innersync/repoly.toml`](./examples/innersync/repoly.toml) — larger multi-product layout  
- [`repoly.toml.example`](./repoly.toml.example)

`plan` / `ctx` rank by tags, id, role, description, light synonyms, and multi-token coverage.

---

## Design principles

- **Local-only** — no telemetry, no accounts  
- **Adapter model** — agents stay single-cwd; repoly chooses *where* and *what context*  
- **Safe defaults** — MCP mutation and shell are explicit opt-in  
- **Stable JSON** — for scripts and agents  
- **Secret-aware skips** — won’t pack `.env*`, `*secret*`, `*credential*` by default  

---

## Development

```bash
cargo test --all
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

| Layer | Location |
|-------|----------|
| Unit | `src/**` |
| CLI e2e | `tests/cli_integration.rs` |
| Lib e2e | `tests/lib_integration.rs` |
| MCP protocol | `tests/mcp_protocol.rs` |

See [CONTRIBUTING.md](./CONTRIBUTING.md). Security: [SECURITY.md](./SECURITY.md).

---

## Roadmap

- `repoly doctor`
- Configurable synonyms / exec policy in `repoly.toml`
- Linux aarch64 release binary
- Homebrew (later)

---

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT license](./LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in `repoly` shall be dual-licensed as above, without any additional terms or conditions.
