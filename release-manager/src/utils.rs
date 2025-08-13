use crate::errors::{ManagerError, ManagerResult};
use colored::*;
use std::path::PathBuf;
use tokio::process::Command;

pub async fn check_app(app: &str) -> ManagerResult<()> {
    let output = Command::new("which")
        .arg(app)
        .output()
        .await?;
        
    if !output.status.success() {
        eprintln!("{} {} !! {} program not found. Please install program to proceed. {}", 
                 "❌".red(), "".red(), app, "".clear());
        return Err(ManagerError::CommandFailed(format!("{} not found", app)));
    }
    
    Ok(())
}

pub fn get_debian_cache_folder() -> PathBuf {
    if let Ok(cache_folder) = std::env::var("DEBIAN_CACHE_FOLDER") {
        PathBuf::from(cache_folder)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".release/debian/cache")
    }
}


pub async fn run_command_with_prefix(prefix: &str, mut cmd: Command) -> ManagerResult<String> {
    let output = cmd.output().await?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ManagerError::CommandFailed(stderr.to_string()));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    
    // Add prefix to each line
    let prefixed_output = stdout
        .lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n");
    
    println!("{}", prefixed_output);
    
    Ok(stdout.to_string())
}

pub async fn run_command_with_debug(mut cmd: Command, debug: bool) -> ManagerResult<String> {
    if debug {
        let program = cmd.as_std().get_program().to_string_lossy();
        let args: Vec<String> = cmd.as_std().get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        let command_line = if args.is_empty() {
            program.to_string()
        } else {
            format!("{} {}", program, args.join(" "))
        };
        println!("{} 🔧 Executing: {}", "".clear(), command_line.cyan());
    }
    
    let output = cmd.output().await?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ManagerError::CommandFailed(stderr.to_string()));
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn format_subcommand_tab() -> &'static str {
    "        "
}

pub fn validate_required_args(args: &[(&str, Option<&String>)]) -> ManagerResult<()> {
    for (name, value) in args {
        if value.is_none() || value.unwrap().is_empty() {
            return Err(ManagerError::MissingParameter(name.to_string()));
        }
    }
    Ok(())
}

pub fn validate_backend(backend: &str) -> ManagerResult<()> {
    match backend {
        "gs" | "hetzner" | "local" => Ok(()),
        _ => Err(ManagerError::UnsupportedBackend(backend.to_string())),
    }
}

pub fn print_operation_info(title: &str, params: &[(&str, &str)]) {
    println!();
    println!(" ℹ️  {} with following parameters:", title);
    for (key, value) in params {
        println!(" - {}: {}", key, value);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_backend() {
        assert!(validate_backend("gs").is_ok());
        assert!(validate_backend("hetzner").is_ok());
        assert!(validate_backend("local").is_ok());
        assert!(validate_backend("invalid").is_err());
    }

    #[test]
    fn test_validate_required_args() {
        let valid_arg = "value".to_string();
        let args = vec![
            ("arg1", Some(&valid_arg)),
            ("arg2", Some(&valid_arg)),
        ];
        assert!(validate_required_args(&args).is_ok());

        let args_with_missing = vec![
            ("arg1", Some(&valid_arg)),
            ("arg2", None),
        ];
        assert!(validate_required_args(&args_with_missing).is_err());
    }
}