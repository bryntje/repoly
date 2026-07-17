# repoly

**Multi-repo workspace awareness for the terminal.**

`repoly` is a local, open-source CLI for polyrepo product stacks. It gives humans and AI coding CLIs (Grok, Claude Code, Codex, Aider, …) something IDEs already have: a machine-readable multi-root workspace with **cross-repo status** and **agent-ready context packs**.

No cloud. No IDE required. No forks of your coding agent.

## Install

```bash
# from source
cargo install --path .

# or build a release binary
cargo build --release
# → target/release/repoly
```

Requires `git` on `PATH`.

## Development & tests

```bash
cargo test              # unit + integration
cargo test -- --nocapture
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

| Layer | Location |
|-------|----------|
| Unit tests | `src/**` (`#[cfg(test)]`) |
| CLI e2e | `tests/cli_integration.rs` (temp multi-repo + `assert_cmd`) |
| Lib e2e | `tests/lib_integration.rs` |
| MCP protocol | `tests/mcp_protocol.rs` (stdio JSON-RPC: initialize, tools/list, tools/call) |
| Fixtures | `tests/common/mod.rs` |

CI runs on GitHub Actions (`.github/workflows/ci.yml`): fmt, clippy, test on macOS + Ubuntu.

## Quickstart

```bash
cd /path/to/your/polyrepo-checkout
repoly init                              # write repoly.toml skeleton
# or:
repoly init --from-code-workspace App.code-workspace

repoly validate
repoly list
repoly status
repoly ctx "payments oauth"
repoly ctx --format prompt "discord link" | pbcopy
```

Point any coding CLI at a single repo **after** reading the pack:

```bash
repoly ctx --format prompt "premium checkout" > /tmp/brief.md
cd innersync-dashboard && grok "Read /tmp/brief.md then fix …"
```

Or inject directly:

```bash
grok -- "$(repoly ctx --format prompt 'premium checkout')"
```

## Commands

| Command | Purpose |
|---------|---------|
| `repoly init` | Create `repoly.toml` (optionally from `.code-workspace`) |
| `repoly validate` | Schema + path checks (`--strict`) |
| `repoly list` | List repos (`--format json`) |
| `repoly status` | Branch / dirty / ahead / behind (`--format json`, `--fetch`) |
| `repoly plan [query]` | Which repos + depends_on order (`markdown` \| `prompt` \| `json`) |
| `repoly ctx [query]` | Context pack (`markdown` \| `prompt` \| `json`) |
| `repoly root` | Print workspace root |
| `repoly path <repo>` | Print absolute path of a repo |
| `repoly exec <repo> -- <cmd…>` | Run a command in one repo cwd |
| `repoly commit <repo> -m "…"` | Safe git commit in workspace repo(s) |
| `repoly run --repos a,b -- <cmd…>` | Run a command across repos |
| `repoly mcp` | MCP stdio server (agent-native tools) |
| `repoly version` | Version |

### MCP server (Grok / Claude / Cursor)

Read-only tools for coding agents — no shell required for status/context:

| Tool | Purpose |
|------|---------|
| `list_repos` | Workspace repo map |
| `status` | Cross-repo git status |
| `plan` | Repo selection + depends_on order |
| `build_context` | Context pack (`prompt` / `markdown` / `json`) |
| `repo_path` | Absolute path of a repo id |
| `workspace_root` | Workspace root |
| `exec` | Run argv in one repo cwd (**opt-in**) |
| `run` | Same command across repos (**opt-in**, needs repos/tags/role) |
| `commit` | Safe git commit in one repo (**opt-in**, same gate as exec) |

**Grok** (`~/.grok/config.toml`) — read-only (default):

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp"]
env = { REPOLY_CONFIG = "/Users/you/Dev/Projects/Github/repoly.toml" }
```

**With exec** (explicit; prefer a repo allowlist):

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp", "--allow-exec", "--exec-repos", "core,app"]
env = { REPOLY_CONFIG = "/Users/you/Dev/Projects/Github/repoly.toml" }
```

Shell for MCP (double opt-in — more powerful, easier to misuse):

```toml
args = ["mcp", "--allow-exec", "--allow-shell", "--exec-repos", "app"]
```

```bash
# CLI equivalents
repoly mcp --allow-exec
repoly mcp --allow-exec --exec-repos core,app
repoly mcp --allow-exec --allow-shell --exec-repos app

grok mcp add repoly -- repoly mcp --allow-exec --exec-repos core,app
```

MCP `run` example (agent tool args):

```json
{
  "repos": "core,app",
  "command": ["git", "rev-parse", "--abbrev-ref", "HEAD"],
  "parallel": false
}
```

Safety notes:

- `exec` / `run` / `commit` are **off** unless `--allow-exec`
- `--exec-repos a,b` further restricts targets (every `run` target must be allowed)
- Default launch is **argv** (`["git","status"]`) — no shell expansion
- `--shell` / MCP `shell: true` needs an extra explicit flag (`--allow-shell` on MCP)
- MCP `run` needs an explicit `repos` / `tags` / `role` filter (never all roots)

### Run commands in repos

```bash
# One repo (stdio inherited — interactive CLIs work)
repoly exec app -- npm test
repoly exec core -- uv run pytest
repoly exec app -- grok "fix the OAuth callback"

# Shell mode (explicit; pipes / && / globs) — off by default
repoly exec app --shell -- 'npm test && echo ok'
repoly run --repos core,app --shell -- 'git status -sb | head -5'

# Several repos, sequential
repoly run --repos core,app -- git status -sb
repoly run --tags backend -- git pull --ff-only

# Parallel batch (captured output, labeled per repo)
repoly run --repos core,app,mind --parallel -- git rev-parse --abbrev-ref HEAD

# Dry-run
repoly run --role frontend --dry-run -- npm run lint
```

Child processes receive:

| Env | Value |
|-----|--------|
| `REPOLY_WORKSPACE` | workspace name |
| `REPOLY_ROOT` | workspace root path |
| `REPOLY_REPO` | repo id |
| `REPOLY_REPO_PATH` | absolute repo path |
| `REPOLY_REPO_ROLE` | role (if set) |

### Useful flags

```bash
repoly status --repos core,app
repoly ctx --tags payments,oauth --format prompt
repoly ctx --role api --no-status
repoly ctx --repos core,app --max-chars 24000
repoly ctx "login checkout" --with-deps    # smarter ranking + depends_on
repoly plan "login"                        # synonyms: login → oauth/auth repos
repoly --help
```

### Query ranking (v0.7+)

`repoly ctx` / `repoly plan` score repos using:

- id / role / tags / description
- lightweight **synonyms** (e.g. `login` → oauth/auth, `billing` → payments)
- snippets of each repo’s `AGENTS.md` / `CLAUDE.md` / `README.md`
- **multi-token coverage** (repos matching more query words rank higher)
- weak floor so noisy one-off matches drop off

Config discovery:

1. `--config <path>`
2. `$REPOLY_CONFIG`
3. Walk up from CWD: `repoly.toml` or `.repoly/repoly.toml`

## Config (`repoly.toml`)

```toml
schema_version = 1

[workspace]
name = "acme"

[context]
always = ["docs/PLATFORM.md", "AGENTS.md"]
status_doc = "docs/STATUS.md"
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
tags = ["frontend"]
depends_on = ["api"]
description = "Web app"
```

See [`repoly.toml.example`](./repoly.toml.example) and [`examples/innersync/repoly.toml`](./examples/innersync/repoly.toml).

## Design principles

- **Local-only** — no telemetry, no accounts
- **Read-only MVP** — status + context never mutate product repos
- **Adapter model** — agents stay single-cwd; `repoly` decides *where* and *what context*
- **Stable JSON** — for scripts and agents
- **Secret-aware skips** — refuses to pack `.env*`, `*secret*`, `*credential*`, key files

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Usage / validation error |
| 2 | Partial success (e.g. some repos missing) |
| 3 | Workspace not found (message text) |

### Plan → context → execute → commit

```bash
repoly plan "identity oauth"
repoly ctx --repos core,styling,app,mind --format prompt "identity oauth"
repoly exec core -- grok "…"

# Commit only in the correct product repo
repoly commit core -m "fix: identity link dual-write"
repoly commit app -m "fix: oauth callback" --all
repoly commit --repos core,app -m "chore: sync related fixes" --all --dry-run
```

`repoly commit` never touches repos outside the workspace map. It will **skip** when nothing is staged (unless you pass `--all` or pathspecs). No force-push; amend/no-verify are explicit flags only.

## Roadmap

- Configurable synonym dictionary in `repoly.toml`
- Optional `repoly.toml` exec policy (default allowlist)

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Please run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all` before opening a PR.

Security reports: [SECURITY.md](./SECURITY.md).

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT license](./LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in `repoly` shall be dual-licensed as above, without any additional terms or conditions.
