# Minimal polyrepo example

Tiny **api + web** layout for trying `repoly` without a real product monorepo.

```text
examples/minimal/
  repoly.toml
  docs/PLATFORM.md
  api/          # pretend backend
  web/          # pretend frontend (depends_on api)
```

## Try it

From this directory (paths are relative to `repoly.toml`):

```bash
cd examples/minimal
repoly validate
repoly list
repoly status          # non-git folders show as n/a — that's fine
repoly plan "oauth"
repoly ctx --format prompt "payments"
```

To exercise real git status, init repos:

```bash
cd api && git init -b main && git add . && git commit -m "init" && cd ..
cd web && git init -b main && git add . && git commit -m "init" && cd ..
repoly status
```

Copy `repoly.toml` to the parent of your own product checkouts and edit paths.
