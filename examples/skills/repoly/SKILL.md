---
name: repoly
description: Multi-repo workspace awareness via the repoly CLI — use before cross-repo tasks
---

# repoly skill

When the user works across multiple git repositories (polyrepo / multi-root workspace):

1. Check for `repoly` on PATH: `repoly version`
2. Prefer workspace commands from the workspace root (or any subdirectory that can walk up to `repoly.toml`):
   - `repoly status` — branch/dirty overview
   - `repoly ctx --format prompt "<task>"` — agent-ready brief
   - `repoly list --format json` — repo map
3. Scope edits to **selected** repos from the context pack unless the user asks otherwise.
4. Commit only inside the correct product repo. Meta/docs folders are context, not product code.
5. Do not dump the entire multi-repo tree into context; use `repoly ctx` with a query or `--repos` / `--tags`.

If `repoly` is missing, tell the user to install it (`cargo install --path .` from the repoly repo) or fall back to reading the workspace's cross-repo docs manually.
