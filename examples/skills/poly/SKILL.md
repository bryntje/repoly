---
name: poly
description: Multi-repo workspace awareness via the poly CLI — use before cross-repo tasks
---

# poly skill

When the user works across multiple git repositories (polyrepo / multi-root workspace):

1. Check for `poly` on PATH: `poly version`
2. Prefer workspace commands from the workspace root (or any subdirectory that can walk up to `poly.toml`):
   - `poly status` — branch/dirty overview
   - `poly ctx --format prompt "<task>"` — agent-ready brief
   - `poly list --format json` — repo map
3. Scope edits to **selected** repos from the context pack unless the user asks otherwise.
4. Commit only inside the correct product repo. Meta/docs folders are context, not product code.
5. Do not dump the entire multi-repo tree into context; use `poly ctx` with a query or `--repos` / `--tags`.

If `poly` is missing, tell the user to install it (`cargo install --path .` from the poly repo) or fall back to reading the workspace's cross-repo docs manually.
