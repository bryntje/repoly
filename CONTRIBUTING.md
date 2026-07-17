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
- Update [CHANGELOG.md](./CHANGELOG.md) under `[Unreleased]` when user-visible behavior changes
- Do not commit secrets, personal `repoly.toml` from private monorepos, or `.env` files

## Code style

- Idiomatic Rust; avoid `unwrap()` outside tests
- Match existing module boundaries (`config`, `status`, `plan`, `ctx`, `run`, `mcp`, …)
- Safety defaults matter: do not weaken MCP allow-exec / allow-shell gates without discussion

## Reporting bugs / ideas

- **Bugs / features:** public GitHub Issues  
- **Security:** see [SECURITY.md](./SECURITY.md)

## Releasing (maintainers)

Binaries are built by [`.github/workflows/release.yml`](./.github/workflows/release.yml) on version tags.

1. Ensure `CHANGELOG.md` has a section for the version and `Cargo.toml` `version` matches (e.g. `0.1.0`).
2. Commit on `master` (green CI).
3. Tag and push:

```bash
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

4. The Release workflow builds:

| Asset | Platform |
|-------|----------|
| `repoly-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `repoly-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `repoly-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 |
| `repoly-aarch64-unknown-linux-gnu.tar.gz` | Linux ARM64 (cross) |
| `repoly-x86_64-pc-windows-msvc.zip` | Windows x86_64 |

5. A GitHub Release is created/updated with archives + `.sha256` sidecars.
6. Optional: re-run for an existing tag via **Actions → Release → Run workflow** and enter the tag.
7. After the public launch checklist: `cargo publish` for crates.io.

Do **not** force-push tags that already have published binaries without a yank/supersede story.

## License

By contributing, you agree that your contributions are licensed under the same terms as the project: **MIT OR Apache-2.0**.
