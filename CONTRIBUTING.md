# Contributing to repoly

Thanks for helping improve multi-repo CLI tooling.

## Prerequisites

- Rust stable (see CI for the tested toolchain)
- `git` on `PATH`
- macOS or Linux recommended; Windows is experimental for now

## Setup

```bash
git clone https://github.com/bryntje/repoly.git
cd repoly
cargo build
cargo test
```

## Development workflow

```bash
# format
cargo fmt --all

# lint (CI uses -D warnings)
cargo clippy --all-targets -- -D warnings

# full suite (unit + CLI e2e + MCP protocol tests)
cargo test --all
```

Please run **fmt**, **clippy**, and **tests** before opening a PR.

## Project layout

| Path | Role |
|------|------|
| `src/` | Library + CLI (`repoly` binary) |
| `tests/` | Integration tests (CLI, lib, MCP) |
| `examples/` | Sample `repoly.toml` workspaces |
| `.github/workflows/` | CI |

## Pull requests

- Keep PRs focused (one concern per PR when practical)
- Prefer small, reviewable commits with complete sentences in messages
- Update `CHANGELOG.md` under `[Unreleased]` when user-visible behavior changes (once that file exists)
- Do not commit secrets, personal `repoly.toml` from private monorepos, or `.env` files

## Code style

- Idiomatic Rust; avoid `unwrap()` outside tests
- Match existing module boundaries (`config`, `status`, `plan`, `ctx`, `run`, `mcp`, …)
- Safety defaults matter: do not weaken MCP allow-exec / allow-shell gates without discussion

## Reporting bugs / ideas

- **Bugs / features:** public GitHub Issues  
- **Security:** see [SECURITY.md](./SECURITY.md)

## License

By contributing, you agree that your contributions are licensed under the same terms as the project: **MIT OR Apache-2.0**.
