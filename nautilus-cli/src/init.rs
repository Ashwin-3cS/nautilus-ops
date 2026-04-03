use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::config::{NautilusConfig, ProjectConfig, Template};
use crate::init_ci;

const GITHUB_ORG: &str = "Ashwin-3cS";

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Template to scaffold (rust, ts, python, or messaging-relayer)
    #[arg(long, value_enum)]
    pub template: Template,

    /// Project directory name. Omit to initialize in the current directory.
    pub project_name: Option<String>,
}

pub async fn run(args: InitArgs) -> Result<()> {
    let template = args.template;
    let repo = template.repo_name();
    let clone_url = format!("https://github.com/{GITHUB_ORG}/{repo}.git");

    // 1. Resolve target directory
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let target = match &args.project_name {
        Some(name) => cwd.join(name),
        None => cwd.clone(),
    };

    // 2. Check directory is empty (or doesn't exist yet)
    if target.exists() {
        let entries: Vec<_> = std::fs::read_dir(&target)
            .with_context(|| format!("Failed to read directory {}", target.display()))?
            .collect();
        if !entries.is_empty() {
            bail!(
                "Directory '{}' is not empty. Use an empty directory or a new project name.",
                target.display()
            );
        }
    }

    println!("{}", "Nautilus Project Init".bold().cyan());
    println!(
        "{} Template: {}",
        "→".cyan(),
        format!("{template}").bold()
    );
    println!(
        "{} Location: {}",
        "→".cyan(),
        target.display().to_string().bold()
    );
    println!("{}", "─".repeat(40).dimmed());

    // 3. Clone template (shallow, no history)
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_message(format!("Cloning {repo}..."));

    let clone_result = Command::new("git")
        .args(["clone", "--depth", "1", &clone_url])
        .arg(&target)
        .output();

    match clone_result {
        Ok(output) if output.status.success() => {
            spinner.finish_and_clear();
            println!(
                "{} Template cloned from github.com/{GITHUB_ORG}/{repo}",
                "✔".green()
            );
        }
        Ok(output) => {
            spinner.finish_and_clear();
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{} git clone failed:\n{stderr}", "✗".red());
        }
        Err(e) => {
            spinner.finish_and_clear();
            bail!(
                "{} Failed to run git: {e}\nIs git installed? Check your internet connection.",
                "✗".red()
            );
        }
    }

    // 4. Remove .git/ so user starts fresh
    let git_dir = target.join(".git");
    if git_dir.exists() {
        std::fs::remove_dir_all(&git_dir)
            .context("Failed to remove .git directory from template")?;
    }

    // 5. Write .nautilus.toml
    let config = NautilusConfig {
        project: ProjectConfig {
            template: Some(template),
        },
        ..Default::default()
    };
    config
        .save(Some(&target))
        .context("Failed to write .nautilus.toml")?;
    println!(
        "{} Config written: .nautilus.toml",
        "✔".green()
    );

    // 6. Run init-ci (cd into target first so Containerfile auto-detection works)
    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&target)
        .context("Failed to change to project directory")?;

    let ci_args = init_ci::InitCiArgs {
        cpu_count: 2,
        memory_mib: 4096,
        dockerfile: PathBuf::from("Dockerfile"),
        output_dir: PathBuf::from(".github/workflows"),
    };
    init_ci::run(ci_args, Some(template)).await?;

    std::env::set_current_dir(original_dir)?;

    // 7. Print next steps
    println!();
    println!("  {}", "Next Steps".bold());
    let mut step = 1;
    if args.project_name.is_some() {
        println!(
            "     {step}. {}",
            format!("cd {}", target.file_name().unwrap().to_string_lossy()).cyan()
        );
        step += 1;
    }
    println!("     {step}. Review and customize the application code");
    step += 1;
    println!("     {step}. Set up a Nitro-enabled EC2 instance and add GitHub Secrets (see above)");
    step += 1;
    println!("     {step}. Push to main to trigger deployment");
    println!();
    println!(
        "     Or build locally:  {}",
        "nautilus build".cyan()
    );
    println!(
        "     Docs: {}",
        "https://github.com/Ashwin-3cS/nautilus-ops".dimmed()
    );
    println!();

    Ok(())
}
