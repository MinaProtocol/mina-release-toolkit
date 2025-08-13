use crate::cli::PersistArgs;
use crate::storage::{StorageBackend, StorageClient, StorageOperations};
use crate::artifacts::{parse_string_list, get_artifact_with_suffix, extract_version_from_deb};
use crate::utils::{validate_required_args, print_operation_info, run_command_with_debug, format_subcommand_tab};
use crate::errors::ManagerResult;
use colored::*;
use tempfile::TempDir;
use tokio::process::Command;

pub async fn execute(args: PersistArgs) -> ManagerResult<()> {
    // Validate required arguments
    validate_required_args(&[
        ("buildkite-build-id", Some(&args.buildkite_build_id)),
        ("target", Some(&args.target)),
        ("codename", Some(&args.codename)),
        ("artifacts", Some(&args.artifacts)),
    ])?;
    
    // Parse lists
    let artifacts = parse_string_list(&args.artifacts);
    
    // Print operation info
    let mut params = vec![
        ("Backend", args.backend.as_str()),
        ("Artifacts", args.artifacts.as_str()),
        ("Buildkite build id", args.buildkite_build_id.as_str()),
        ("Codename", args.codename.as_str()),
        ("Suite", args.suite.as_str()),
        ("Target", args.target.as_str()),
    ];
    
    if let Some(ref new_version) = args.new_version {
        params.push(("New version", new_version));
    }
    
    print_operation_info("Persisting mina artifacts", &params);
    
    // Set up storage
    let backend = StorageBackend::from_str(&args.backend)?;
    let storage = StorageClient::new(backend);
    
    // Create temporary directory
    let tmp_dir = TempDir::new()?;
    println!(" - Using temporary directory: {}", tmp_dir.path().display());
    println!();
    
    // Process each artifact
    for artifact in &artifacts {
        let remote_path = format!(
            "{}/{}/debians/{}/{}_*",
            storage.backend.root_path(),
            args.buildkite_build_id,
            args.codename,
            artifact
        );
        
        // Download artifacts to temp directory
        storage.download(&remote_path, tmp_dir.path().to_str().unwrap()).await?;
        
        // If new version is specified, rebuild the package
        if let Some(ref new_version) = args.new_version {
            // Find the downloaded deb file
            let mut entries = tokio::fs::read_dir(tmp_dir.path()).await?;
            let mut deb_file = None;
            
            while let Ok(Some(entry)) = entries.next_entry().await {
                let file_name = entry.file_name();
                let file_name_str = file_name.to_string_lossy();
                if file_name_str.starts_with(&format!("{}_", artifact)) && file_name_str.ends_with(".deb") {
                    deb_file = Some(entry.path());
                    break;
                }
            }
            
            if let Some(deb_path) = deb_file {
                let source_version = extract_version_from_deb(&deb_path.file_name().unwrap().to_string_lossy())?;
                let artifact_full_name = get_artifact_with_suffix(artifact, None);
                
                println!(" 🗃️  Rebuilding {} debian from {} to {}", artifact, source_version, new_version);
                
                // Use reversion command
                let mut cmd = Command::new("bash");
                cmd.arg("-c")
                   .arg(format!(
                       "reversion --deb {} --package {} --source-version {} --new-version {} --suite unstable --new-suite {} --new-name {}",
                       deb_path.display(),
                       artifact_full_name,
                       source_version,
                       new_version,
                       args.suite,
                       artifact_full_name
                   ));
                
                if args.debug {
                    run_command_with_debug(cmd, true).await?;
                } else {
                    crate::utils::run_command_with_prefix(format_subcommand_tab(), cmd).await?;
                }
            }
        }
        
        // Upload to target location
        let target_path = format!(
            "{}/{}/debians/{}/",
            storage.backend.root_path(),
            args.target,
            args.codename
        );
        
        let local_pattern = format!("{}/*{}*", tmp_dir.path().display(), artifact);
        storage.upload(&local_pattern, &target_path).await?;
    }
    
    println!("{}", " ✅  Done.".green());
    Ok(())
}