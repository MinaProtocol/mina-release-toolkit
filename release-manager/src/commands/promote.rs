use crate::cli::PromoteArgs;
use crate::artifacts::{parse_artifact_list, parse_string_list, get_artifact_with_suffix, calculate_debian_version, calculate_docker_tag, get_suffix, Artifact};
use crate::utils::{validate_required_args, print_operation_info};
use crate::errors::ManagerResult;
use crate::docker_promote::promote_docker_image;
use crate::verification::{verify_docker_image, verify_debian_package};
use crate::reversion;
use colored::*;

pub async fn execute(args: PromoteArgs) -> ManagerResult<()> {
    // Validate required arguments
    validate_required_args(&[
        ("target-version", Some(&args.target_version)),
        ("source-version", Some(&args.source_version)),
    ])?;
    
    if !args.only_dockers {
        validate_required_args(&[
            ("source-channel", args.source_channel.as_ref()),
            ("target-channel", args.target_channel.as_ref()),
        ])?;
    }
    
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
    
    let mut params = vec![
        ("Promoting artifacts", args.artifacts.as_str()),
        ("Networks", args.networks.as_str()),
        ("Promoting codenames", args.codenames.as_str()),
        ("Publish to docker.io", publish_to_docker_io_str.as_str()),
        ("Only dockers", only_dockers_str.as_str()),
        ("Only debians", only_debians_str.as_str()),
        ("Verify", verify_str.as_str()),
        ("Dry run", dry_run_str.as_str()),
        ("Strip network from archive", strip_network_str.as_str()),
    ];
    
    if !args.only_dockers {
        if let Some(ref source_channel) = args.source_channel {
            params.push(("Source channel", source_channel.as_str()));
        }
        if let Some(ref target_channel) = args.target_channel {
            params.push(("Target channel", target_channel.as_str()));
        }
        params.push(("Source version", args.source_version.as_str()));
        params.push(("Target version", args.target_version.as_str()));
    }
    
    print_operation_info("Promoting mina artifacts", &params);
    
    // Warning if source and target versions are the same
    if args.source_version == args.target_version {
        println!(" ⚠️  Warning: Source version and target version are the same.");
        println!("    Script will do promotion but it won't have an effect at the end unless you are publishing dockers from gcr.io to docker.io ...");
        println!();
    }
    
    // Process each artifact
    for artifact in &artifacts {
        for codename in &codenames {
            match artifact {
                Artifact::MinaLogproc => {
                    if !args.only_dockers {
                        promote_debian(
                            artifact.as_str(),
                            codename,
                            &args.source_version,
                            &args.target_version,
                            args.source_channel.as_deref().unwrap(),
                            args.target_channel.as_deref().unwrap(),
                            None,
                            args.verify,
                            args.dry_run,
                            &args.debian_repo,
                            args.debian_sign_key.as_deref(),
                            args.debug,
                        ).await?;
                    }
                    
                    if !args.only_debians {
                        println!("   ℹ️  There is no mina-logproc docker image to promote. skipping");
                    }
                }
                
                Artifact::MinaArchive => {
                    for network in &networks {
                        if !args.only_dockers {
                            promote_debian(
                                artifact.as_str(),
                                codename,
                                &args.source_version,
                                &args.target_version,
                                args.source_channel.as_deref().unwrap(),
                                args.target_channel.as_deref().unwrap(),
                                Some(network),
                                args.verify,
                                args.dry_run,
                                &args.debian_repo,
                                args.debian_sign_key.as_deref(),
                                args.debug,
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
                                args.debug,
                            ).await?;
                        }
                    }
                }
                
                Artifact::MinaRosetta | Artifact::MinaDaemon => {
                    for network in &networks {
                        if !args.only_dockers {
                            promote_debian(
                                artifact.as_str(),
                                codename,
                                &args.source_version,
                                &args.target_version,
                                args.source_channel.as_deref().unwrap(),
                                args.target_channel.as_deref().unwrap(),
                                Some(network),
                                args.verify,
                                args.dry_run,
                                &args.debian_repo,
                                args.debian_sign_key.as_deref(),
                                args.debug,
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
                                args.debug,
                            ).await?;
                        }
                    }
                }
            }
        }
    }
    
    println!("{}", " ✅  Promoting done.".green());
    Ok(())
}

async fn promote_debian(
    artifact: &str,
    codename: &str,
    source_version: &str,
    target_version: &str,
    source_channel: &str,
    target_channel: &str,
    network: Option<&str>,
    verify: bool,
    dry_run: bool,
    debian_repo: &str,
    debian_sign_key: Option<&str>,
    _debug: bool,
) -> ManagerResult<()> {
    println!(" 🍥 Promoting {} debian from {} to {}, from {} to {}", 
             artifact, source_channel, target_channel, source_version, target_version);
    println!("    📦 Target debian version: {}", 
             calculate_debian_version(artifact, target_version, codename, network));
    
    let artifact_full_name = get_artifact_with_suffix(artifact, network);
    
    if !dry_run {
        println!("    🗃️  Promoting {} debian from {}/{} to {}/{}", 
                 artifact, codename, source_version, codename, target_version);
        
        // Use Rust reversion instead of shell script
        let deb_path = std::path::Path::new(debian_repo).join(&format!("{}.deb", artifact_full_name));
        
        reversion::reversion_debian_package(
            &deb_path,
            &artifact_full_name,
            source_version,
            target_version,
            source_channel,
            target_channel,
            Some(&artifact_full_name),
        ).await?;
        
        if verify {
            println!("     📋 Verifying: {} debian to {} channel with {} version", 
                     artifact, target_channel, target_version);
            
            verify_debian_package(
                &artifact_full_name,
                target_version,
                debian_repo,
                codename,
                target_channel,
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
    _debug: bool,
) -> ManagerResult<()> {
    let network_suffix = get_suffix(artifact, Some(network));
    let artifact_full_source_version = format!("{}-{}{}", source_version, codename, network_suffix);
    let artifact_full_target_version = format!("{}-{}{}", target_version, codename, network_suffix);
    
    println!(" 🐋 Publishing {} docker for '{}' network and '{}' codename with '{}' version", 
             artifact, network, codename, target_version);
    println!("    📦 Target version: {}", 
             calculate_docker_tag(publish_to_docker_io, artifact, target_version, codename, Some(network)));
    println!();
    
    if !dry_run {
        promote_docker_image(
            artifact,
            &artifact_full_source_version,
            &artifact_full_target_version,
            publish_to_docker_io,
            true, // quiet mode (equivalent to -q flag)
        ).await?;
        println!();
        
        if verify {
            println!("    📋 Verifying: {} docker for '{}' network and '{}' codename with '{}' version", 
                     artifact, network, codename, target_version);
            println!();
            
            let repo = if publish_to_docker_io { "docker.io/minaprotocol" } else { "gcr.io/o1labs-192920" };
            
            verify_docker_image(
                artifact,
                target_version,
                repo,
                codename,
                &network_suffix,
            ).await?;
            
            println!();
        }
    }
    
    Ok(())
}