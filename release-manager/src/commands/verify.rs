use crate::cli::VerifyArgs;
use crate::artifacts::{parse_artifact_list, parse_string_list, get_artifact_with_suffix, calculate_docker_tag, combine_docker_suffixes, get_repo, Artifact};
use crate::utils::print_operation_info;
use crate::errors::ManagerResult;
use crate::verification::{verify_debian_package, verify_docker_image};
use colored::*;

pub async fn execute(args: VerifyArgs) -> ManagerResult<()> {
    // Parse lists
    let artifacts = parse_artifact_list(&args.artifacts)?;
    let networks = parse_string_list(&args.networks);
    let codenames = parse_string_list(&args.codenames);
    
    // Print operation info
    let docker_io_str = args.docker_io.to_string();
    let signed_debian_repo_str = args.signed_debian_repo.to_string();
    let only_debians_str = args.only_debians.to_string();
    let only_dockers_str = args.only_dockers.to_string();
    let docker_suffix_str = args.docker_suffix.as_deref().unwrap_or("");
    
    let params = vec![
        ("Verifying artifacts", args.artifacts.as_str()),
        ("Networks", args.networks.as_str()),
        ("Version", args.version.as_str()),
        ("Promoting codenames", args.codenames.as_str()),
        ("Published to docker.io", docker_io_str.as_str()),
        ("Debian repo", args.debian_repo.as_str()),
        ("Debian repos is signed", signed_debian_repo_str.as_str()),
        ("Channel", args.channel.as_str()),
        ("Only debians", only_debians_str.as_str()),
        ("Only dockers", only_dockers_str.as_str()),
        ("Docker suffix", docker_suffix_str),
    ];
    
    print_operation_info("Verifying mina artifacts", &params);
    
    let repo = get_repo(args.docker_io);
    
    // Process each artifact
    for artifact in &artifacts {
        for codename in &codenames {
            match artifact {
                Artifact::MinaLogproc => {
                    if !args.only_dockers {
                        println!("     📋  Verifying: {} debian on {} channel with {} version for {} codename", 
                                 artifact.as_str(), args.channel, args.version, codename);
                        
                        verify_debian(
                            artifact.as_str(),
                            &args.version,
                            codename,
                            &args.debian_repo,
                            &args.channel,
                            args.signed_debian_repo,
                            args.debug,
                        ).await?;
                    }
                    
                    if !args.only_debians {
                        println!("    ℹ️  There is no mina-logproc docker image. skipping");
                    }
                }
                
                Artifact::MinaArchive => {
                    for network in &networks {
                        let artifact_full_name = get_artifact_with_suffix(artifact.as_str(), Some(network));
                        let docker_suffix_combined = combine_docker_suffixes(network, args.docker_suffix.as_deref());
                        
                        if !args.only_dockers {
                            println!("     📋  Verifying: {} debian on {} channel with {} version for {} codename", 
                                     artifact.as_str(), args.channel, args.version, codename);
                            
                            verify_debian(
                                &artifact_full_name,
                                &args.version,
                                codename,
                                &args.debian_repo,
                                &args.channel,
                                args.signed_debian_repo,
                                args.debug,
                            ).await?;
                            println!();
                        }
                        
                        if !args.only_debians {
                            println!("      📋  Verifying: {} docker on {}", 
                                     artifact.as_str(), 
                                     calculate_docker_tag(args.docker_io, artifact.as_str(), &args.version, codename, Some(network)));
                            
                            verify_docker(
                                artifact.as_str(),
                                &args.version,
                                codename,
                                &docker_suffix_combined,
                                repo,
                                args.debug,
                            ).await?;
                            println!();
                        }
                    }
                }
                
                Artifact::MinaRosetta => {
                    for network in &networks {
                        let artifact_full_name = get_artifact_with_suffix(artifact.as_str(), Some(network));
                        let docker_suffix_combined = combine_docker_suffixes(network, args.docker_suffix.as_deref());
                        
                        if !args.only_dockers {
                            println!("     📋  Verifying: {} debian on {} channel with {} version for {} codename", 
                                     artifact_full_name, args.channel, args.version, codename);
                            println!();
                            
                            verify_debian(
                                &artifact_full_name,
                                &args.version,
                                codename,
                                &args.debian_repo,
                                &args.channel,
                                args.signed_debian_repo,
                                args.debug,
                            ).await?;
                            println!();
                        }
                        
                        if !args.only_debians {
                            println!("      📋  Verifying: {} docker on {}", 
                                     artifact.as_str(),
                                     calculate_docker_tag(args.docker_io, &artifact_full_name, &args.version, codename, None));
                            println!();
                            
                            verify_docker(
                                artifact.as_str(),
                                &args.version,
                                codename,
                                &docker_suffix_combined,
                                repo,
                                args.debug,
                            ).await?;
                            println!();
                        }
                    }
                }
                
                Artifact::MinaDaemon => {
                    for network in &networks {
                        let artifact_full_name = get_artifact_with_suffix(artifact.as_str(), Some(network));
                        let docker_suffix_combined = combine_docker_suffixes(network, args.docker_suffix.as_deref());
                        
                        if !args.only_dockers {
                            println!("     📋  Verifying: {} debian on {} channel with {} version for {} codename", 
                                     artifact_full_name, args.channel, args.version, codename);
                            println!();
                            
                            verify_debian(
                                &artifact_full_name,
                                &args.version,
                                codename,
                                &args.debian_repo,
                                &args.channel,
                                args.signed_debian_repo,
                                args.debug,
                            ).await?;
                            println!();
                        }
                        
                        if !args.only_debians {
                            println!("      📋  Verifying: {} docker on {}", 
                                     artifact.as_str(),
                                     calculate_docker_tag(args.docker_io, &artifact_full_name, &args.version, codename, None));
                            println!();
                            
                            verify_docker(
                                artifact.as_str(),
                                &args.version,
                                codename,
                                &docker_suffix_combined,
                                repo,
                                args.debug,
                            ).await?;
                            println!();
                        }
                    }
                }
            }
        }
    }
    
    println!("{}", " ✅  Verification done.".green());
    Ok(())
}

async fn verify_debian(
    artifact: &str,
    version: &str,
    codename: &str,
    debian_repo: &str,
    channel: &str,
    signed: bool,
    _debug: bool,
) -> ManagerResult<()> {
    verify_debian_package(artifact, version, debian_repo, codename, channel, signed).await
}

async fn verify_docker(
    artifact: &str,
    version: &str,
    codename: &str,
    suffix: &str,
    repo: &str,
    _debug: bool,
) -> ManagerResult<()> {
    verify_docker_image(artifact, version, repo, codename, suffix).await
}