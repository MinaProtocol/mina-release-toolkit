use crate::cli::PullArgs;
use crate::storage::{StorageBackend, StorageClient, StorageOperations};
use crate::artifacts::{parse_string_list, get_artifact_with_suffix};
use crate::utils::{validate_required_args, print_operation_info};
use crate::errors::ManagerResult;
use colored::*;

pub async fn execute(args: PullArgs) -> ManagerResult<()> {
    // Validate required arguments
    validate_required_args(&[
        ("buildkite-build-id", Some(&args.buildkite_build_id)),
    ])?;
    
    // Parse lists
    let artifacts = parse_string_list(&args.artifacts);
    let codenames = parse_string_list(&args.codenames);
    let networks = parse_string_list(&args.networks);
    
    // Print operation info
    let params = vec![
        ("Backend", args.backend.as_str()),
        ("Artifacts", args.artifacts.as_str()),
        ("Buildkite build id", args.buildkite_build_id.as_str()),
        ("Target", args.target.as_str()),
        ("Codenames", args.codenames.as_str()),
        ("Networks", args.networks.as_str()),
    ];
    
    print_operation_info("Pulling mina artifacts", &params);
    
    // Set up storage
    let backend = StorageBackend::from_str(&args.backend)?;
    let storage = StorageClient::new(backend);
    
    // Process each combination of artifact, codename, and network
    for artifact in &artifacts {
        for codename in &codenames {
            for network in &networks {
                println!("  📥  Pulling {} for {} codename and {} network", artifact, codename, network);
                
                let artifact_full_name = get_artifact_with_suffix(artifact, Some(network));
                let remote_path = format!(
                    "{}/{}/debians/{}/{}_*",
                    storage.backend.root_path(),
                    args.buildkite_build_id,
                    codename,
                    artifact_full_name
                );
                
                // Download to target directory
                storage.download(&remote_path, &args.target).await?;
            }
        }
    }
    
    println!("{}", " ✅  Done.".green());
    Ok(())
}