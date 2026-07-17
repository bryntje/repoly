# Security Policy

## Supported versions

Security fixes are applied to the latest release on the default branch (`master` / `main`). There is no long-term support branch yet.

## Scope

`repoly` is a **local** CLI and optional MCP stdio server. It:

- Reads workspace config and repository files you point it at
- Runs `git` and optional child commands (`exec` / `run` / `commit`)
- Does **not** phone home or require a cloud account

It is a **power tool**, not an OS sandbox. Anyone who can start the process and choose flags/config already has the privileges of that user account.

### Threat model

**Trust boundary:** the process that launches `repoly` (a human or an MCP host config under that user’s control).

| In scope | Out of scope |
|----------|----------------|
| Accidental over-broad agent mutation after opt-in flags | Remote attackers (no network service) |
| Pathspecs escaping a repo root via `commit` | Malicious local user who already owns the machine |
| Sensitive binaries via MCP `exec`/`run` | Host editor tools that write files without repoly |
| Secrets pulled into context packs by filename | Supply-chain behavior inside an *allowed* binary (e.g. `npm` postinstall) |
| Shell mode bypassing basename policy | Multi-tenant / multi-user isolation |

Threats we care about most:

- Unexpected command execution via MCP when `--allow-exec` / `--allow-shell` is enabled
- Path traversal or writing outside intended repo roots
- Accidental inclusion of secrets in context packs

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security-sensitive reports.

Prefer one of:

1. **GitHub Security Advisories** for [bryntje/repoly](https://github.com/bryntje/repoly) (Private vulnerability reporting), if enabled  
2. Email the maintainer via the address on the GitHub profile [@bryntje](https://github.com/bryntje)

Include:

- Description of the issue and impact
- Steps to reproduce
- Affected version / commit if known
- Whether a fix or PoC is included

We will acknowledge receipt when possible and coordinate disclosure after a fix is available.

## Safety layers

### Layer 1 — mutation gates (shipped since 0.1)

| Control | Behavior |
|---------|----------|
| Default read-only MCP | `exec` / `run` / `commit` off until `--allow-exec` |
| Shell double opt-in | `shell=true` needs `--allow-shell` |
| Repo allowlist | `--exec-repos a,b` limits which workspace repo ids may be targeted |
| `run` requires a filter | Refuses “all repos” without `--repos` / `--tags` / `--role` |
| Commit helper | Fixed `git` argv, no shell; skips empty commits |

### Layer 2 — policy (path + command)

| Control | Behavior |
|---------|----------|
| Commit path confinement | Pathspecs must resolve under the target repo root (always on) |
| Default bin deny (MCP + `--allow-exec`) | Blocks sensitive basenames: `sudo`, `doas`, `su`, `pkexec`, `dd`, `mkfs*`, `diskutil`, `diskpart`, `format`, `shutdown`, `reboot`, `poweroff`, `halt` |
| Optional bin allow | `--exec-bin-allow git,cargo,npm,…` — only those basenames |
| Optional extra deny | `--exec-bin-deny …` merged with defaults |
| Disable default deny | `--no-default-exec-deny` |
| Shell vs bin policy | If any bin policy is active (including default deny), `shell=true` is **rejected** so scripts cannot smuggle denied tools |
| Capture limits | `--exec-timeout-secs`, `--exec-max-output-bytes` (or `[policy]` in toml) |
| Audit log | `--audit-log` / `[policy].audit_log` — JSONL for exec/run/commit |
| Context skip globs | `[policy].skip_globs` + built-in secret filename heuristics |

**Not default-denied:** everyday tools such as `rm`, `git`, `npm`, `cargo` — ecosystems differ; use `--exec-bin-allow` for a tight agent profile, or add `rm` to `--exec-bin-deny` if you want that guardrail.

Plain CLI `repoly exec` / `run` (no MCP) is unrestricted unless you later opt into workspace `[policy]` (planned); humans typing commands are outside the agent threat model.

### Context packing

- Skips common secret filenames (e.g. `.env*`, `*secret*`, `*credential*`, `*.pem` / `*.key`)
- Configurable skip globs are planned / additive — still filename-based, not full secret scanning

## Recommended MCP host profile (generic)

Neutral polyrepo example (`api` / `web` / `worker` are placeholders for your repo ids):

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
  # default deny list still applies underneath the allowlist
]
env = { REPOLY_CONFIG = "/absolute/path/to/repoly.toml" }
```

Hardening checklist:

1. Prefer **argv** arrays; avoid `--allow-shell` unless you pass `--no-default-exec-deny` and accept the risk  
2. Set `--exec-repos` to the repos the agent may change for that session  
3. Prefer `--exec-bin-allow` for agent hosts  
4. Set a **timeout** and **max output** for agent sessions  
5. Optional **audit log** for forensics  
6. Review diffs before push; repoly does not replace code review  
7. Remember host tools (file edit, host shell) are a separate surface

## Residual risk

Even with both layers:

- An allowed binary can still do harmful work (`git push --force`, `npm publish`, …)
- cwd is the repo, but processes keep the user’s full privileges (network, home directory, credentials)
- Host MCP clients may expose other write tools that never go through repoly

Still: treat agent-driven `exec`/`run` as powerful. Prefer allowlists and review diffs before push.
