# poly

**Multi-repo workspace awareness for the terminal.**

`poly` is a local, open-source CLI for polyrepo product stacks. It gives humans and AI coding CLIs (Grok, Claude Code, Codex, Aider, …) something IDEs already have: a machine-readable multi-root workspace with **cross-repo status** and **agent-ready context packs**.

No cloud. No IDE required. No forks of your coding agent.

## Install

```bash
# from source
cargo install --path .

# or build a release binary
cargo build --release
# → target/release/poly
```

Requires `git` on `PATH`.

## Quickstart

```bash
cd /path/to/your/polyrepo-checkout
poly init                              # write poly.toml skeleton
# or:
poly init --from-code-workspace App.code-workspace

poly validate
poly list
poly status
poly ctx "payments oauth"
poly ctx --format prompt "discord link" | pbcopy
```

Point any coding CLI at a single repo **after** reading the pack:

```bash
poly ctx --format prompt "premium checkout" > /tmp/brief.md
cd innersync-dashboard && grok "Read /tmp/brief.md then fix …"
```

Or inject directly:

```bash
grok -- "$(poly ctx --format prompt 'premium checkout')"
```

## Commands

| Command | Purpose |
|---------|---------|
| `poly init` | Create `poly.toml` (optionally from `.code-workspace`) |
| `poly validate` | Schema + path checks (`--strict`) |
| `poly list` | List repos (`--format json`) |
| `poly status` | Branch / dirty / ahead / behind (`--format json`, `--fetch`) |
| `poly plan [query]` | Which repos + depends_on order (`markdown` \| `prompt` \| `json`) |
| `poly ctx [query]` | Context pack (`markdown` \| `prompt` \| `json`) |
| `poly root` | Print workspace root |
| `poly path <repo>` | Print absolute path of a repo |
| `poly exec <repo> -- <cmd…>` | Run a command in one repo cwd |
| `poly commit <repo> -m "…"` | Safe git commit in workspace repo(s) |
| `poly run --repos a,b -- <cmd…>` | Run a command across repos |
| `poly mcp` | MCP stdio server (agent-native tools) |
| `poly version` | Version |

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
| `exec` | Run argv in a repo cwd (**opt-in**) |
| `commit` | Safe git commit in one repo (**opt-in**, same gate as exec) |

**Grok** (`~/.grok/config.toml`) — read-only (default):

```toml
[mcp_servers.poly]
command = "poly"
args = ["mcp"]
env = { POLY_CONFIG = "/Users/you/Dev/Projects/Github/poly.toml" }
```

**With exec** (explicit; prefer a repo allowlist):

```toml
[mcp_servers.poly]
command = "poly"
args = ["mcp", "--allow-exec", "--exec-repos", "core,app"]
env = { POLY_CONFIG = "/Users/you/Dev/Projects/Github/poly.toml" }
```

Shell for MCP (double opt-in — more powerful, easier to misuse):

```toml
args = ["mcp", "--allow-exec", "--allow-shell", "--exec-repos", "app"]
```

```bash
# CLI equivalents
poly mcp --allow-exec
poly mcp --allow-exec --exec-repos core,app
poly mcp --allow-exec --allow-shell --exec-repos app

grok mcp add poly -- poly mcp --allow-exec --exec-repos core,app
```

Safety notes:

- `exec` is **off** unless `--allow-exec`
- `--exec-repos a,b` further restricts targets
- Default launch is **argv** (`["git","status"]`) — no shell expansion
- `--shell` / MCP `shell: true` needs an extra explicit flag (`--allow-shell` on MCP)
- `poly run` multi-repo remains CLI-only (no MCP multi-run yet)

### Run commands in repos

```bash
# One repo (stdio inherited — interactive CLIs work)
poly exec app -- npm test
poly exec core -- uv run pytest
poly exec app -- grok "fix the OAuth callback"

# Shell mode (explicit; pipes / && / globs) — off by default
poly exec app --shell -- 'npm test && echo ok'
poly run --repos core,app --shell -- 'git status -sb | head -5'

# Several repos, sequential
poly run --repos core,app -- git status -sb
poly run --tags backend -- git pull --ff-only

# Parallel batch (captured output, labeled per repo)
poly run --repos core,app,mind --parallel -- git rev-parse --abbrev-ref HEAD

# Dry-run
poly run --role frontend --dry-run -- npm run lint
```

Child processes receive:

| Env | Value |
|-----|--------|
| `POLY_WORKSPACE` | workspace name |
| `POLY_ROOT` | workspace root path |
| `POLY_REPO` | repo id |
| `POLY_REPO_PATH` | absolute repo path |
| `POLY_REPO_ROLE` | role (if set) |

### Useful flags

```bash
poly status --repos core,app
poly ctx --tags payments,oauth --format prompt
poly ctx --role api --no-status
poly ctx --repos core,app --max-chars 24000
poly ctx "login checkout" --with-deps    # smarter ranking + depends_on
poly plan "login"                        # synonyms: login → oauth/auth repos
poly --help
```

### Query ranking (v0.7+)

`poly ctx` / `poly plan` score repos using:

- id / role / tags / description
- lightweight **synonyms** (e.g. `login` → oauth/auth, `billing` → payments)
- snippets of each repo’s `AGENTS.md` / `CLAUDE.md` / `README.md`
- **multi-token coverage** (repos matching more query words rank higher)
- weak floor so noisy one-off matches drop off

Config discovery:

1. `--config <path>`
2. `$POLY_CONFIG`
3. Walk up from CWD: `poly.toml` or `.poly/poly.toml`

## Config (`poly.toml`)

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

See [`poly.toml.example`](./poly.toml.example) and [`examples/innersync/poly.toml`](./examples/innersync/poly.toml).

## Design principles

- **Local-only** — no telemetry, no accounts
- **Read-only MVP** — status + context never mutate product repos
- **Adapter model** — agents stay single-cwd; `poly` decides *where* and *what context*
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
poly plan "identity oauth"
poly ctx --repos core,styling,app,mind --format prompt "identity oauth"
poly exec core -- grok "…"

# Commit only in the correct product repo
poly commit core -m "fix: identity link dual-write"
poly commit app -m "fix: oauth callback" --all
poly commit --repos core,app -m "chore: sync related fixes" --all --dry-run
```

`poly commit` never touches repos outside the workspace map. It will **skip** when nothing is staged (unless you pass `--all` or pathspecs). No force-push; amend/no-verify are explicit flags only.

## Roadmap

- Optional MCP multi-repo `run`
- Configurable synonym dictionary in `poly.toml`

## License

MIT OR Apache-2.0
