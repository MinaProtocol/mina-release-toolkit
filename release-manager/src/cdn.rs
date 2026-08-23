//! Shared helpers for talking to the APT repository's S3 bucket and the
//! CloudFront distribution in front of it.
//!
//! Both `validate` and `upload` need the same two things: turn a repository
//! address into the bucket name `deb-s3` expects, and invalidate the CDN so
//! readers do not keep serving a stale `Packages` index. They lived in
//! `commands/validate.rs` first; they are here so the second caller reuses
//! them instead of copying them.

use crate::errors::{ManagerError, ManagerResult};
use crate::process::CommandExecutor;

/// The S3 bucket name `deb-s3` expects, given a repository address.
///
/// Production passes the host name (`stable.apt.packages.minaprotocol.com`),
/// which doubles as the bucket, and it is returned unchanged. Tests against
/// MinIO pass an endpoint plus a bucket path (`http://127.0.0.1:9000/test-bucket`);
/// the bucket is the first path segment and the endpoint is supplied separately
/// through [`crate::process::S3Config`].
pub fn bucket_name(debian_repo: &str) -> String {
    if let Some(after_scheme) = debian_repo.split_once("://") {
        let rest = after_scheme.1.trim_end_matches('/');
        if let Some((_, path)) = rest.split_once('/') {
            return path.split('/').next().unwrap_or("").to_string();
        }
        return rest.to_string();
    }
    debian_repo.to_string()
}

/// Invalidate the CloudFront cache for one codename's `dists/` prefix.
///
/// Deliberately best-effort: a repository with no CNAME, or no distribution
/// behind it, is a valid configuration (a bare bucket), and must not fail an
/// upload that already succeeded. Only a `create-invalidation` that cannot be
/// spawned at all is reported as an error.
pub fn invalidate_cloudfront(
    exec: &dyn CommandExecutor,
    debian_repo: &str,
    codename: &str,
) -> ManagerResult<()> {
    let dig_out = exec.run("dig", &["+short", "CNAME", debian_repo]);
    let cf_domain = match dig_out {
        Ok(out) => out.stdout.trim().trim_end_matches('.').to_string(),
        Err(_) => String::new(),
    };
    if cf_domain.is_empty() {
        println!(
            "    ⚠️  No CNAME found for {} — skipping CDN invalidation",
            debian_repo
        );
        return Ok(());
    }

    let query = format!("DistributionList.Items[?DomainName=='{}'].Id", cf_domain);
    let list_out = exec.run(
        "aws",
        &[
            "cloudfront",
            "list-distributions",
            "--query",
            &query,
            "--output",
            "text",
        ],
    );
    let dist_id = match list_out {
        Ok(out) => out.stdout.trim().to_string(),
        Err(_) => String::new(),
    };
    if dist_id.is_empty() || dist_id == "None" {
        println!("    ⚠️  Could not find CloudFront distribution");
        return Ok(());
    }

    let paths = format!("/dists/{}/*", codename);
    let out = exec
        .run(
            "aws",
            &[
                "cloudfront",
                "create-invalidation",
                "--distribution-id",
                &dist_id,
                "--paths",
                &paths,
            ],
        )
        .map_err(|e| {
            ManagerError::ValidationError(format!("aws cloudfront create-invalidation: {}", e))
        })?;

    for line in out.stdout.lines() {
        println!("    {}", line);
    }
    for line in out.stderr.lines() {
        eprintln!("    {}", line);
    }
    if out.is_success() {
        println!("    ✅ Cache invalidation submitted");
    } else {
        println!("    ⚠️  Cache invalidation command failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{CommandOutput, MockCommandExecutor};

    #[test]
    fn bucket_name_passes_through_a_bare_host() {
        assert_eq!(
            bucket_name("stable.apt.packages.minaprotocol.com"),
            "stable.apt.packages.minaprotocol.com"
        );
    }

    #[test]
    fn bucket_name_extracts_the_path_segment_from_an_endpoint_url() {
        assert_eq!(
            bucket_name("http://127.0.0.1:9000/test-bucket"),
            "test-bucket"
        );
        assert_eq!(
            bucket_name("http://127.0.0.1:9000/test-bucket/nested"),
            "test-bucket"
        );
    }

    #[test]
    fn bucket_name_handles_an_endpoint_url_without_a_path() {
        assert_eq!(bucket_name("http://127.0.0.1:9000"), "127.0.0.1:9000");
    }

    #[test]
    fn invalidation_is_skipped_when_the_repo_has_no_cname() {
        let exec = MockCommandExecutor::new();
        exec.expect("dig", |_| true, CommandOutput::success(""));

        invalidate_cloudfront(&exec, "bare-bucket", "bullseye").unwrap();

        let calls = exec.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "must not reach aws: {:?}", *calls);
    }

    #[test]
    fn invalidation_submits_the_dists_prefix_for_the_codename() {
        let exec = MockCommandExecutor::new();
        exec.expect(
            "dig",
            |_| true,
            CommandOutput::success("d123.cloudfront.net.\n"),
        );
        exec.expect(
            "aws",
            |a| a.contains(&"list-distributions"),
            CommandOutput::success("E2XYZ\n"),
        );
        exec.expect(
            "aws",
            |a| a.contains(&"create-invalidation"),
            CommandOutput::success("{}"),
        );

        invalidate_cloudfront(&exec, "stable.apt.packages.minaprotocol.com", "noble").unwrap();

        let calls = exec.calls.lock().unwrap();
        let inv = calls
            .iter()
            .find(|c| c.args.contains(&"create-invalidation".to_string()))
            .expect("no create-invalidation call");
        assert!(
            inv.args.contains(&"/dists/noble/*".to_string()),
            "wrong paths: {:?}",
            inv.args
        );
        assert!(
            inv.args.contains(&"E2XYZ".to_string()),
            "wrong distribution id: {:?}",
            inv.args
        );
    }
}
