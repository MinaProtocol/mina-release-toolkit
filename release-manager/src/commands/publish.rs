use crate::cli::PublishArgs;
use crate::storage::{StorageBackend, StorageClient, get_cached_debian_or_download};
use crate::artifacts::{parse_artifact_list, parse_string_list, get_artifact_with_suffix, calculate_debian_version, calculate_docker_tag, Artifact};
use crate::utils::{get_debian_cache_folder, validate_required_args, validate_backend, print_operation_info};
use crate::errors::ManagerResult;
use crate::reversion::reversion_debian_package;
use crate::debian_publish::publish_debian_package;
use crate::verification::{verify_debian_package, verify_docker_image};
use crate::docker_promote::promote_docker_image;
use colored::*;
use std::env;

pub async fn execute(args: PublishArgs) -> ManagerResult<()> {
    // Validate required arguments
    validate_required_args(&[
        ("target-version", Some(&args.target_version)),
        ("source-version", Some(&args.source_version)),
        ("buildkite-build-id", Some(&args.buildkite_build_id)),
        ("channel", Some(&args.channel)),
    ])?;
    
    validate_backend(&args.backend)?;
    
    // Parse lists
    let artifacts = parse_artifact_list(&args.artifacts)?;
    let networks = parse_string_list(&args.networks);
    let codenames = parse_string_list(&args.codenames);
    
    // Print operation info
    let publish_to_docker_io_str = args.publish_to_docker_io.to_string();
    let only_dockers_str = args.only_dockers.to_string();
    let only_debians_str = args.only_debians.to_string();
    let verify_str = args.verify.to_string();
    let dry_run_str = args.dry_run.to_string();
    let strip_network_str = args.strip_network_from_archive.to_string();
    let debian_sign_key_str = args.debian_sign_key.as_deref().unwrap_or("");
    
    let params = vec![
        ("Publishing artifacts", args.artifacts.as_str()),
        ("Publishing networks", args.networks.as_str()),
        ("Buildkite build id", args.buildkite_build_id.as_str()),
        ("Source version", args.source_version.as_str()),
        ("Target version", args.target_version.as_str()),
        ("Publishing codenames", args.codenames.as_str()),
        ("Target channel", args.channel.as_str()),
        ("Publish to docker.io", publish_to_docker_io_str.as_str()),
        ("Only dockers", only_dockers_str.as_str()),
        ("Only debians", only_debians_str.as_str()),
        ("Verify", verify_str.as_str()),
        ("Dry run", dry_run_str.as_str()),
        ("Backend", args.backend.as_str()),
        ("Debian repo", args.debian_repo.as_str()),
        ("Debian sign key", debian_sign_key_str),
        ("Strip network from archive", strip_network_str.as_str()),
    ];
    
    print_operation_info("Publishing mina artifacts", &params);
    
    // Set up storage
    let backend = StorageBackend::from_str(&args.backend)?;
    let storage = StorageClient::new(backend);
    
    // Set environment variable for buildkite build id
    env::set_var("BUILDKITE_BUILD_ID", &args.buildkite_build_id);
    
    let cache_folder = get_debian_cache_folder();
    tokio::fs::create_dir_all(&cache_folder).await?;
    
    // Process each artifact
    for artifact in &artifacts {
        for codename in &codenames {
            match artifact {
                Artifact::MinaLogproc => {
                    if !args.only_dockers {
                        publish_debian(
                            &storage,
                            artifact.as_str(),
                            codename,
                            &args.source_version,
                            &args.target_version,
                            &args.channel,
                            None,
                            args.verify,
                            args.dry_run,
                            &args.debian_repo,
                            args.debian_sign_key.as_deref(),
                            None,
                            &args.buildkite_build_id,
                            args.debug
                        ).await?;
                    }
                    
                    if !args.only_debians {
                        println!("ℹ️  There is no {} docker image to publish. skipping", artifact.as_str());
                    }
                }
                
                Artifact::MinaArchive => {
                    for network in &networks {
                        let new_name = if args.strip_network_from_archive {
                            Some("mina-archive")
                        } else {
                            None
                        };
                        
                        if !args.only_dockers {
                            publish_debian(
                                &storage,
                                artifact.as_str(),
                                codename,
                                &args.source_version,
                                &args.target_version,
                                &args.channel,
                                Some(network),
                                args.verify,
                                args.dry_run,
                                &args.debian_repo,
                                args.debian_sign_key.as_deref(),
                                new_name,
                                &args.buildkite_build_id,
                                args.debug
                            ).await?;
                        }
                        
                        if !args.only_debians {
                            promote_and_verify_docker(
                                artifact.as_str(),
                                &args.source_version,
                                &args.target_version,
                                codename,
                                network,
                                args.publish_to_docker_io,
                                args.verify,
                                args.dry_run,
                            ).await?;
                        }
                    }
                }
                
                Artifact::MinaRosetta | Artifact::MinaDaemon => {
                    for network in &networks {
                        if !args.only_dockers {
                            publish_debian(
                                &storage,
                                artifact.as_str(),
                                codename,
                                &args.source_version,
                                &args.target_version,
                                &args.channel,
                                Some(network),
                                args.verify,
                                args.dry_run,
                                &args.debian_repo,
                                args.debian_sign_key.as_deref(),
                                None,
                                &args.buildkite_build_id,
                                args.debug
                            ).await?;
                        }
                        
                        if !args.only_debians {
                            promote_and_verify_docker(
                                artifact.as_str(),
                                &args.source_version,
                                &args.target_version,
                                codename,
                                network,
                                args.publish_to_docker_io,
                                args.verify,
                                args.dry_run,
                            ).await?;
                        }
                    }
                }
            }
        }
    }
    
    println!("{}", " ✅  Publishing done.".green());
    Ok(())
}

async fn publish_debian(
    storage: &StorageClient,
    artifact: &str,
    codename: &str,
    source_version: &str,
    target_version: &str,
    channel: &str,
    network: Option<&str>,
    verify: bool,
    dry_run: bool,
    debian_repo: &str,
    debian_sign_key: Option<&str>,
    new_artifact_name: Option<&str>,
    buildkite_build_id: &str,
    debug: bool,
) -> ManagerResult<()> {
    // Download the debian package to cache
    let cache_folder = get_debian_cache_folder();
    get_cached_debian_or_download(storage, artifact, codename, network, buildkite_build_id, &cache_folder).await?;
    
    let artifact_full_name = get_artifact_with_suffix(artifact, network);
    
    let new_name = new_artifact_name.unwrap_or(&artifact_full_name);
    
    // Build reversion command if needed
    if source_version != target_version {
        println!(" 🗃️  Rebuilding {} debian from {} to {}", artifact, source_version, target_version);
        
        // Find the actual .deb file that matches the pattern
        let deb_files = tokio::fs::read_dir(cache_folder.join(codename)).await?;
        let mut found_deb_path = None;
        
        let mut entries = deb_files;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.starts_with(&format!("{}_", artifact_full_name)) && filename.ends_with(".deb") {
                    found_deb_path = Some(path);
                    break;
                }
            }
        }
        
        let actual_deb_path = found_deb_path.ok_or_else(|| {
            crate::errors::ManagerError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Could not find .deb file matching pattern: {}_*.deb in {}",
                    artifact_full_name,
                    cache_folder.join(codename).display()
                ),
            ))
        })?;
        
        // Use Rust reversion implementation
        let new_deb_path = reversion_debian_package(
            &actual_deb_path,
            &artifact_full_name,
            source_version,
            target_version,
            "unstable",
            channel,
            Some(new_name),
        ).await?;
        
        println!(" ✅ Debian package reversioned: {}", new_deb_path.display());
    }
    
    println!(" 🍥  Publishing {} debian to {} channel with {} version", artifact, channel, target_version);
    println!("     📦  Target debian version: {}", calculate_debian_version(artifact, target_version, codename, network));
    
    if !dry_run {
        // Use Rust implementation for Debian package publishing
        let package_path = cache_folder.join(codename).join(format!("{}_{}.deb", new_name, target_version));
        
        publish_debian_package(
            &package_path.to_string_lossy(),
            target_version,
            debian_repo,
            codename,
            channel,
            debian_sign_key,
            debug
        ).await?;
        
        if verify {
            println!("     📋 Verifying: {} debian to {} channel with {} version", new_name, channel, target_version);
            
            verify_debian_package(
                new_name,
                target_version,
                debian_repo,
                codename,
                channel,
                debian_sign_key.is_some(),
            ).await?;
        }
    }
    
    Ok(())
}

async fn promote_and_verify_docker(
    artifact: &str,
    source_version: &str,
    target_version: &str,
    codename: &str,
    network: &str,
    publish_to_docker_io: bool,
    verify: bool,
    dry_run: bool,
) -> ManagerResult<()> {
    use crate::artifacts::get_suffix;
    
    let network_suffix = get_suffix(artifact, Some(network));
    let artifact_full_source_version = format!("{}-{}{}", source_version, codename, network_suffix);
    let artifact_full_target_version = format!("{}-{}{}", target_version, codename, network_suffix);
    
    println!(" 🐋 Publishing {} docker for '{}' network and '{}' codename with '{}' version", artifact, network, codename, target_version);
    println!("    📦 Target version: {}", calculate_docker_tag(publish_to_docker_io, artifact, target_version, codename, Some(network)));
    println!();
    
    if !dry_run {
        // Use Rust implementation for Docker image promotion
        promote_docker_image(
            artifact,
            &artifact_full_source_version,
            &artifact_full_target_version,
            publish_to_docker_io,
            false, // not quiet
        ).await?;
        
        if verify {
            println!("    📋 Verifying: {} docker for '{}' network and '{}' codename with '{}' version", artifact, network, codename, target_version);
            
            let repo = if publish_to_docker_io { "docker.io/minaprotocol" } else { "gcr.io/o1labs-192920" };
            let full_version = format!("{}-{}{}", target_version, codename, network_suffix);
            
            verify_docker_image(
                artifact,
                &full_version,
                repo,
                codename,
                &network_suffix,
            ).await?;
        }
    }
    
    Ok(())
}