# repoly MCP setup

`repoly mcp` speaks [Model Context Protocol](https://modelcontextprotocol.io) over **stdio**.

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

With mutation (prefer an allowlist):

```toml
[mcp_servers.repoly]
command = "repoly"
args = ["mcp", "--allow-exec", "--exec-repos", "api,web"]
env = { REPOLY_CONFIG = "/absolute/path/to/your/repoly.toml" }
```

Shell for agents (double opt-in):

```toml
args = ["mcp", "--allow-exec", "--allow-shell", "--exec-repos", "api,web"]
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

1. `plan` with a short task query  
2. `build_context` (`format=prompt`) for selected repos  
3. Edit only those repos  
4. Optional: `commit` / `exec` if you enabled mutation  

## Safety

- Mutation tools are disabled unless `--allow-exec`
- `--exec-repos a,b` restricts targets
- `shell=true` requires `--allow-shell`
- `run` never targets all repos without a filter
