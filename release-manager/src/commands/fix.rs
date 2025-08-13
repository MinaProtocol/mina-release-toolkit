use crate::cli::FixArgs;
use crate::artifacts::parse_string_list;
use crate::utils::{print_operation_info, run_command_with_debug};
use crate::errors::ManagerResult;
use colored::*;
use tokio::process::Command;

pub async fn execute(args: FixArgs) -> ManagerResult<()> {
    // Parse lists
    let codenames = parse_string_list(&args.codenames);
    
    // Print operation info
    let params = vec![
        ("Codenames", args.codenames.as_str()),
        ("Channel", args.channel.as_str()),
    ];
    
    print_operation_info("Fixing debian repository", &params);
    
    let bucket_arg = "--bucket=packages.o1test.net";
    let s3_region_arg = "--s3-region=us-west-2";
    
    // Fix manifests for each codename
    for codename in &codenames {
        let mut cmd = Command::new("deb-s3");
        cmd.arg("verify")
           .arg("--fix-manifests")
           .arg(bucket_arg)
           .arg(s3_region_arg)
           .arg(format!("--codename={}", codename))
           .arg(format!("--component={}", args.channel));
        
        if args.debug {
            run_command_with_debug(cmd, true).await?;
        } else {
            let output = cmd.output().await?;
            
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("Failed to fix manifests for {}: {}", codename, stderr);
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout);
                println!("Fixed manifests for {}: {}", codename, stdout);
            }
        }
    }
    
    println!("{}", " ✅  Done.".green());
    Ok(())
}