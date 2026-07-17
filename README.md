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
| `poly ctx [query]` | Context pack (`markdown` \| `prompt` \| `json`) |
| `poly root` | Print workspace root |
| `poly version` | Version |

### Useful flags

```bash
poly status --repos core,app
poly ctx --tags payments,oauth --format prompt
poly ctx --role api --no-status
poly ctx --repos core,app --max-chars 24000
poly --help   # global help via clap subcommands: poly status --help
```

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

## Roadmap (not in v0.1)

- `poly exec <repo> -- <cmd>`
- `poly run --repos a,b -- <cli…>`
- Optional MCP server
- Smarter query ranking

## License

MIT OR Apache-2.0
