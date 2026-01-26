use std::fs;
use std::path::PathBuf;
use std::process::Command;

use clap::{Parser, Subcommand};
use regex::Regex;
use similar::TextDiff;

#[derive(Parser)]
#[command(author, version, about = "Schema maintenance helpers", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    /// Show tracked schema metadata
    Show,
    /// Download the tracked schema JSON to schema/pipeline.schema.json
    Download,
    /// Compare tracked schema to upstream main
    Compare,
    /// Regenerate schema bindings by running cargo test
    Generate,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.command {
        CommandKind::Show => show_schema_info(),
        CommandKind::Download => download_schema(),
        CommandKind::Compare => compare_schema(),
        CommandKind::Generate => regenerate_schema(),
    }
}

fn show_schema_info() -> anyhow::Result<()> {
    let info = schema_info()?;
    println!(
        "Tracked schema:\n  Commit: {}\n  Date: {}\n  Repo: {}\n  URL: {}",
        info.commit, info.date, info.repo, info.url
    );

    let path = PathBuf::from("schema/pipeline.schema.json");
    let metadata = fs::metadata(&path)?;
    println!("Local file: {} ({} bytes)", path.display(), metadata.len());
    Ok(())
}

fn download_schema() -> anyhow::Result<()> {
    let info = schema_info()?;
    println!("Downloading {}", info.url);
    let body = ureq::get(&info.url).call()?.into_string()?;
    fs::write("schema/pipeline.schema.json", body)?;
    println!("schema/pipeline.schema.json updated.");
    Ok(())
}

fn compare_schema() -> anyhow::Result<()> {
    let info = schema_info()?;
    let local = fs::read_to_string("schema/pipeline.schema.json")?;
    let remote_url = format!(
        "https://raw.githubusercontent.com/{}/main/schema.json",
        info.repo.trim_start_matches("https://github.com/")
    );
    println!("Fetching upstream schema from {}", remote_url);
    let remote = ureq::get(&remote_url).call()?.into_string()?;

    if local == remote {
        println!("No differences detected.");
        return Ok(());
    }

    println!("Differences detected (local vs upstream main):");
    let diff = TextDiff::from_lines(&local, &remote);
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => "-",
            similar::ChangeTag::Insert => "+",
            similar::ChangeTag::Equal => " ",
        };
        print!("{}{}", sign, change);
    }
    Ok(())
}

fn regenerate_schema() -> anyhow::Result<()> {
    let status = Command::new("cargo").args(["test", "--lib"]).status()?;
    if !status.success() {
        anyhow::bail!("cargo test failed with status {status}");
    }
    Ok(())
}

struct SchemaInfo {
    commit: String,
    date: String,
    repo: String,
    url: String,
}

fn schema_info() -> anyhow::Result<SchemaInfo> {
    let contents = fs::read_to_string("src/version.rs")?;
    let commit = capture_const(&contents, "SCHEMA_COMMIT")?;
    let date = capture_const(&contents, "SCHEMA_DATE")?;
    let repo = capture_const(&contents, "SCHEMA_REPO")?;
    let url = capture_const(&contents, "SCHEMA_URL")?;
    Ok(SchemaInfo {
        commit,
        date,
        repo,
        url,
    })
}

fn capture_const(contents: &str, name: &str) -> anyhow::Result<String> {
    let pattern = format!(r#"{}: &str = \"([^\"]+)\""#, name);
    let re = Regex::new(&pattern)?;
    let caps = re
        .captures(contents)
        .ok_or_else(|| anyhow::anyhow!("{} not found", name))?;
    Ok(caps[1].to_string())
}
