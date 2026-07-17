mod cli;
mod config;
mod context;
mod discover;
mod status;

use anyhow::{bail, Context as _, Result};
use clap::Parser;
use cli::{Cli, Commands, CtxFormat, ListFormat, StatusFormat};
use config::{find_config, load_config, ConfigError};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            let is_not_found = err
                .chain()
                .any(|e| matches!(e.downcast_ref::<ConfigError>(), Some(ConfigError::NotFound)));
            if is_not_found {
                ExitCode::from(3)
            } else {
                ExitCode::from(1)
            }
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init {
            from_code_workspace,
            force,
        } => {
            cmd_init(from_code_workspace, force)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Validate { strict, config } => {
            let (cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let warnings = workspace.validate(strict);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            if strict && !warnings.is_empty() {
                bail!(
                    "validation failed with {} warning(s) in strict mode",
                    warnings.len()
                );
            }
            let mut errors = 0usize;
            for repo in &workspace.repos {
                let p = workspace.repo_path(repo);
                if !p.exists() {
                    eprintln!(
                        "error: repo '{}' path does not exist: {}",
                        repo.id,
                        p.display()
                    );
                    errors += 1;
                }
            }
            for doc in &workspace.context.always {
                let p = workspace.root.join(doc);
                if !p.exists() {
                    eprintln!("warning: always-doc missing: {}", p.display());
                    if strict {
                        errors += 1;
                    }
                }
            }
            if errors > 0 {
                bail!(
                    "validation failed ({errors} error(s)); config: {}",
                    cfg_path.display()
                );
            }
            println!(
                "ok: {} ({} repos)",
                cfg_path.display(),
                workspace.repos.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        Commands::List { format, config } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            match format {
                ListFormat::Table => {
                    println!(
                        "{:<20} {:<8} {:<6} {:<6} {}",
                        "ID", "ROLE", "EXISTS", "GIT", "PATH"
                    );
                    for repo in &workspace.repos {
                        let path = workspace.repo_path(repo);
                        let exists = path.exists();
                        let is_git = exists && path.join(".git").exists();
                        println!(
                            "{:<20} {:<8} {:<6} {:<6} {}",
                            repo.id,
                            repo.role.as_deref().unwrap_or("-"),
                            if exists { "yes" } else { "no" },
                            if is_git { "yes" } else { "no" },
                            path.display()
                        );
                    }
                }
                ListFormat::Json => {
                    let rows: Vec<_> = workspace
                        .repos
                        .iter()
                        .map(|repo| {
                            let path = workspace.repo_path(repo);
                            let exists = path.exists();
                            serde_json::json!({
                                "id": repo.id,
                                "path": path,
                                "role": repo.role,
                                "tags": repo.tags,
                                "description": repo.description,
                                "depends_on": repo.depends_on,
                                "exists": exists,
                                "is_git": exists && path.join(".git").exists(),
                            })
                        })
                        .collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "workspace": workspace.name,
                            "root": workspace.root,
                            "repos": rows,
                        }))?
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Status {
            repos,
            format,
            fetch,
            config,
        } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let filter = parse_csv(repos.as_deref());
            let report = status::collect_status(&workspace, filter.as_deref(), fetch);
            match format {
                StatusFormat::Table => status::print_table(&report),
                StatusFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
            if report.repos.iter().any(|r| r.error.is_some() || !r.exists) {
                Ok(ExitCode::from(2))
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
        Commands::Ctx {
            query,
            repos,
            tags,
            role,
            format,
            max_chars,
            no_status,
            config,
        } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let pack = context::build_context(
                &workspace,
                query.as_deref(),
                parse_csv(repos.as_deref()).as_deref(),
                parse_csv(tags.as_deref()).as_deref(),
                role.as_deref(),
                max_chars,
                no_status,
            )?;
            match format {
                CtxFormat::Markdown => print!("{}", context::format_markdown(&pack)),
                CtxFormat::Prompt => print!("{}", context::format_prompt(&pack)),
                CtxFormat::Json => println!("{}", serde_json::to_string_pretty(&pack)?),
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Root { config } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            println!("{}", workspace.root.display());
            Ok(ExitCode::SUCCESS)
        }
        Commands::Version => {
            println!("poly {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn resolve_workspace(config_flag: Option<&PathBuf>) -> Result<(PathBuf, config::Workspace)> {
    let cfg_path = if let Some(p) = config_flag {
        p.clone()
    } else {
        find_config().map_err(|e| match e {
            ConfigError::NotFound => ConfigError::NotFound,
            other => other,
        })?
    };

    let workspace =
        load_config(&cfg_path).with_context(|| format!("loading {}", cfg_path.display()))?;
    Ok((cfg_path, workspace))
}

fn parse_csv(s: Option<&str>) -> Option<Vec<String>> {
    s.map(|v| {
        v.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
            .collect()
    })
    .filter(|v: &Vec<String>| !v.is_empty())
}

fn cmd_init(from_code_workspace: Option<PathBuf>, force: bool) -> Result<()> {
    let out = PathBuf::from("poly.toml");
    if out.exists() && !force {
        bail!("poly.toml already exists (use --force to overwrite)");
    }

    let content = if let Some(ws_path) = from_code_workspace {
        let raw = std::fs::read_to_string(&ws_path)
            .with_context(|| format!("reading {}", ws_path.display()))?;
        discover::poly_toml_from_code_workspace(&raw, &ws_path)?
    } else {
        discover::default_poly_toml().to_string()
    };

    std::fs::write(&out, content).with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
