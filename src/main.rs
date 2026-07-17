use anyhow::{bail, Context as _, Result};
use clap::Parser;
use repoly::cli::{Cli, Commands, CtxFormat, ListFormat, PlanFormat, StatusFormat};
use repoly::commit;
use repoly::config::{find_config, load_config, ConfigError};
use repoly::context;
use repoly::discover;
use repoly::mcp;
use repoly::plan;
use repoly::run;
use repoly::status;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run_cli() {
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

fn run_cli() -> Result<ExitCode> {
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
                        "{:<20} {:<8} {:<6} {:<6} PATH",
                        "ID", "ROLE", "EXISTS", "GIT"
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
        Commands::Plan {
            query,
            repos,
            tags,
            role,
            no_deps,
            format,
            no_status,
            config,
        } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let work = plan::build_plan(
                &workspace,
                query.as_deref(),
                parse_csv(repos.as_deref()).as_deref(),
                parse_csv(tags.as_deref()).as_deref(),
                role.as_deref(),
                !no_deps,
                no_status,
            )?;
            match format {
                PlanFormat::Markdown => print!("{}", plan::format_markdown(&work)),
                PlanFormat::Prompt => print!("{}", plan::format_prompt(&work)),
                PlanFormat::Json => println!("{}", serde_json::to_string_pretty(&work)?),
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Ctx {
            query,
            repos,
            tags,
            role,
            format,
            max_chars,
            no_status,
            with_deps,
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
                with_deps,
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
        Commands::Path { repo, config } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let entry = run::resolve_repo(&workspace, &repo)?;
            let path = workspace.repo_path(entry);
            if !path.exists() {
                bail!("repo '{repo}' path does not exist: {}", path.display());
            }
            println!("{}", path.display());
            Ok(ExitCode::SUCCESS)
        }
        Commands::Commit {
            repo,
            message,
            all,
            paths,
            repos,
            tags,
            role,
            amend,
            allow_empty,
            no_verify,
            signoff,
            dry_run,
            config,
        } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let targets = commit::resolve_targets(
                &workspace,
                repo.as_deref(),
                parse_csv(repos.as_deref()).as_deref(),
                parse_csv(tags.as_deref()).as_deref(),
                role.as_deref(),
            )?;
            let opts = commit::CommitOpts {
                message,
                all,
                paths,
                amend,
                allow_empty,
                no_verify,
                dry_run,
                signoff,
            };
            let results = commit::commit_many(&workspace, &targets, &opts)?;
            for r in &results {
                if !r.stdout.is_empty() {
                    print!("{}", r.stdout);
                }
                if !r.stderr.is_empty() && !r.success {
                    eprint!("{}", r.stderr);
                }
            }
            commit::print_results(&results);
            Ok(ExitCode::from(commit::exit_code(&results)))
        }
        Commands::Exec {
            repo,
            dry_run,
            shell,
            config,
            cmd,
        } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let entry = run::resolve_repo(&workspace, &repo)?;
            let mode = run::LaunchMode::from_shell_flag(shell);
            let result = run::exec_one(&workspace, entry, &cmd, dry_run, mode)?;
            if let Some(err) = &result.error {
                bail!("exec failed in '{}': {err}", result.repo_id);
            }
            let code = result.code().unwrap_or(1) as u8;
            Ok(ExitCode::from(code))
        }
        Commands::Run {
            repos,
            tags,
            role,
            parallel,
            continue_on_error,
            dry_run,
            shell,
            config,
            cmd,
        } => {
            let (_cfg_path, workspace) = resolve_workspace(config.as_ref())?;
            let selected = run::select_repos(
                &workspace,
                parse_csv(repos.as_deref()).as_deref(),
                parse_csv(tags.as_deref()).as_deref(),
                role.as_deref(),
            )?;
            let mode = run::LaunchMode::from_shell_flag(shell);
            let results = run::run_many(
                &workspace,
                &selected,
                &cmd,
                parallel,
                continue_on_error,
                dry_run,
                mode,
            )?;
            run::summarize(&results);
            Ok(ExitCode::from(run::exit_code_from_results(&results)))
        }
        Commands::Mcp {
            config,
            allow_exec,
            exec_repos,
            allow_shell,
            exec_bin_allow,
            exec_bin_deny,
            no_default_exec_deny,
            exec_timeout_secs,
            exec_max_output_bytes,
            audit_log,
        } => {
            let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
            rt.block_on(mcp::serve(mcp::McpOptions {
                config,
                allow_exec,
                exec_repos: parse_csv(exec_repos.as_deref()),
                allow_shell,
                exec_bin_allow: parse_csv(exec_bin_allow.as_deref()),
                exec_bin_deny: parse_csv(exec_bin_deny.as_deref()),
                no_default_exec_deny,
                exec_timeout_secs,
                exec_max_output_bytes,
                audit_log,
            }))?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Version => {
            println!("repoly {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn resolve_workspace(
    config_flag: Option<&PathBuf>,
) -> Result<(PathBuf, repoly::config::Workspace)> {
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
    let out = PathBuf::from("repoly.toml");
    if out.exists() && !force {
        bail!("repoly.toml already exists (use --force to overwrite)");
    }

    let content = if let Some(ws_path) = from_code_workspace {
        let raw = std::fs::read_to_string(&ws_path)
            .with_context(|| format!("reading {}", ws_path.display()))?;
        discover::repoly_toml_from_code_workspace(&raw, &ws_path)?
    } else {
        discover::default_repoly_toml().to_string()
    };

    std::fs::write(&out, content).with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
