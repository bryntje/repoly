# Security Policy

## Supported versions

Security fixes are applied to the latest release on the default branch (`master` / `main`). There is no long-term support branch yet.

## Scope

`repoly` is a **local** CLI and optional MCP stdio server. It:

- Reads workspace config and repository files you point it at
- Runs `git` and optional child commands (`exec` / `run` / `commit`)
- Does **not** phone home or require a cloud account

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

## Safe defaults

- MCP mutation tools (`exec`, `run`, `commit`) require explicit `--allow-exec`
- Shell form requires an additional `--allow-shell`
- `--exec-repos` can restrict targets
- Context packing skips common secret filenames (e.g. `.env*`, `*secret*`, `*credential*`)

Still: treat agent-driven `exec`/`run` as powerful. Prefer allowlists and review diffs before push.
