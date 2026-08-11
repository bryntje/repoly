# repoly MCP setup

`repoly mcp` speaks [Model Context Protocol](https://modelcontextprotocol.io) over **stdio**.

If you run `repoly mcp` yourself in a terminal, it will look “stuck” (cursor on the next line). That is normal: the process is **waiting for an MCP client** on stdin. Wire it into Grok / Claude / Cursor instead; use Ctrl-C to stop a manual run. A short explanation is printed to **stderr** when stdin is a TTY.

## Tools

| Tool | Default | Purpose |
|------|---------|---------|
| `list_repos` | on | Workspace map |
| `status` | on | Cross-repo git status |
| `plan` | on | Repo selection + depends_on order |
| `build_context` | on | Context pack for agents |
| `repo_path` | on | Absolute path of a repo id |
| `workspace_root` | on | Workspace root |
| `exec` | **off** | Run argv in one repo (`--allow-exec`) |
| `run` | **off** | Same command across repos (`--allow-exec` + filter) |
| `commit` | **off** | Git commit in one repo (`--allow-exec`) |

## Grok

`~/.grok/config.toml` (read-only):

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp"]
env = { REPOLY_CONFIG = "/absolute/path/to/your/repoly.toml" }
```

With mutation (prefer a repo allowlist; default sensitive-bin deny is on):

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp", "--allow-exec", "--exec-repos", "api,web"]
env = { REPOLY_CONFIG = "/absolute/path/to/your/repoly.toml" }
```

Hardened agent host (recommended):

```toml
[mcp_servers.repoly]
command = "repoly"
args = [
  "mcp",
  "--allow-exec",
  "--exec-repos", "api,web,worker",
  "--exec-bin-allow", "git,cargo,npm,pnpm,yarn,node,python,python3,go,make",
  "--exec-timeout-secs", "120",
  "--exec-max-output-bytes", "262144",
  "--audit-log", "/tmp/repoly-audit.jsonl",
]
env = { REPOLY_CONFIG = "/absolute/path/to/your/repoly.toml" }
```

Optional workspace defaults in `repoly.toml` (flags still override):

```toml
[policy]
skip_globs = ["**/.npmrc", "**/*service-account*.json"]
use_builtin_secret_filters = true
exec_timeout_secs = 120
exec_max_output_bytes = 262144
# audit_log = ".repoly/audit.jsonl"

# Optional plan/ctx synonym groups (merged with built-ins)
# [ranking]
# synonym_groups = [
#   ["billing", "invoice", "stripe"],
# ]
# Optional phrase rewrites (inject tokens when phrase appears in the query)
# [[ranking.rewrites]]
# match = "invoice portal"
# add = ["billing", "stripe"]
```

Shell for agents (double opt-in). **Bin policy must be inactive** for `shell=true` to work — pass `--no-default-exec-deny` and do not set bin allow/deny lists:

```toml
args = [
  "mcp",
  "--allow-exec",
  "--allow-shell",
  "--no-default-exec-deny",
  "--exec-repos", "api,web",
]
```

CLI helper:

```bash
grok mcp add repoly -- repoly mcp
# or:
grok mcp add repoly -- repoly mcp --allow-exec --exec-repos api,web
```

## Claude Code

Add a stdio MCP server that runs `repoly mcp` with the same flags/env as above (see Claude Code MCP docs for your config file format). Point `REPOLY_CONFIG` at the workspace `repoly.toml`.

## Cursor / other MCP hosts

Same pattern:

- **command:** `repoly` (must be on `PATH`)
- **args:** `["mcp"]` or with allow-exec flags
- **env:** `REPOLY_CONFIG=/absolute/path/to/repoly.toml`

## Agent workflow

1. `plan` with a short task query (`format=grok` or `prompt`)  
2. `build_context` (`format=grok` or `prompt`) for **narrow** selected repos; raise `max_chars` when always-docs are large (often 90000–120000)  
3. Check pack `budget` / tips — if `repo_files_included` is 0, open AGENTS.md in the target repo yourself  
4. Edit only those repos  
5. Optional: `commit` / `exec` if you enabled mutation  

`repoly doctor` on the CLI reports always-doc size vs work-mode budget and **info** suggestions for sibling git dirs not listed in `repoly.toml`.

## Safety

### Layer 1

- Mutation tools are disabled unless `--allow-exec`
- `--exec-repos a,b` restricts targets
- `shell=true` requires `--allow-shell`
- `run` never targets all repos without a filter

### Layer 2

- **Default bin deny** when `--allow-exec` is set: blocks sensitive basenames (`sudo`, `dd`, `mkfs*`, `shutdown`, …). Disable with `--no-default-exec-deny`.
- **Optional** `--exec-bin-allow` / `--exec-bin-deny` (basenames, case-insensitive)
- **Shell + bin policy:** if any bin policy is active (including default deny), `shell=true` is rejected so scripts cannot bypass basename checks
- **Commit pathspecs** must stay under the target repo root
- **`--exec-timeout-secs` / `--exec-max-output-bytes`** — optional capture limits (also via `[policy]`)
- **`--audit-log`** — JSONL of mutation tool calls
- **`[policy].skip_globs`** — extra paths excluded from context packs

See [SECURITY.md](../SECURITY.md) for the full threat model and residual risk.
