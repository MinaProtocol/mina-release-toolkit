//! Env-var configuration. Loaded at startup so misconfiguration
//! fails fast — not at upload time the way the Python tool did.

use anyhow::{anyhow, Result};
use std::env;

pub const ENV_HOST: &str = "INFLUX_HOST";
pub const ENV_TOKEN: &str = "INFLUX_TOKEN";
pub const ENV_ORG: &str = "INFLUX_ORG";
pub const ENV_BUCKET: &str = "INFLUX_BUCKET_NAME";

#[derive(Debug, Clone)]
pub struct InfluxConfig {
    pub host: String,
    pub token: String,
    pub org: String,
    pub bucket: String,
}

impl InfluxConfig {
    /// Read all four env vars. Returns the first one that's missing,
    /// so users see a single useful error per missing variable rather
    /// than a generic "configuration error".
    pub fn from_env() -> Result<Self> {
        let host = required(ENV_HOST)?;
        let token = required(ENV_TOKEN)?;
        let org = required(ENV_ORG)?;
        let bucket = required(ENV_BUCKET)?;
        Ok(Self {
            host: normalize_host(&host),
            token,
            org,
            bucket,
        })
    }
}

fn required(var: &str) -> Result<String> {
    env::var(var).map_err(|_| anyhow!("{} env var not defined", var))
}

/// Auto-prepend `https://` if the host isn't already a full URL —
/// matches the Python tool's behaviour and saves the caller from
/// having to remember which form the deploy expected.
fn normalize_host(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{}", raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_full_https_url() {
        assert_eq!(normalize_host("https://x.example"), "https://x.example");
    }

    #[test]
    fn normalize_keeps_http_url() {
        assert_eq!(normalize_host("http://x.example"), "http://x.example");
    }

    #[test]
    fn normalize_adds_https_when_missing() {
        assert_eq!(normalize_host("x.example"), "https://x.example");
    }
}
