use anyhow::{anyhow, Result};
use std::io::{Read, Write};

/// Compression formats Debian's `data.tar.*` and `control.tar.*` use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Xz,
    Zstd,
}

impl Compression {
    /// Infer compression from a `data.tar.*` / `control.tar.*` filename.
    pub fn from_filename(name: &str) -> Result<Self> {
        if name.ends_with(".tar") {
            Ok(Compression::None)
        } else if name.ends_with(".tar.gz") {
            Ok(Compression::Gzip)
        } else if name.ends_with(".tar.xz") {
            Ok(Compression::Xz)
        } else if name.ends_with(".tar.zst") {
            Ok(Compression::Zstd)
        } else {
            Err(anyhow!("Unknown compression for {}", name))
        }
    }

    pub fn as_keyword(&self) -> &'static str {
        match self {
            Compression::None => "none",
            Compression::Gzip => "gz",
            Compression::Xz => "xz",
            Compression::Zstd => "zst",
        }
    }

    pub fn from_keyword(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(Compression::None),
            "gz" => Ok(Compression::Gzip),
            "xz" => Ok(Compression::Xz),
            "zst" => Ok(Compression::Zstd),
            _ => Err(anyhow!("Unknown compression keyword: {}", s)),
        }
    }

    /// Filename suffix added to `data.tar` / `control.tar`.
    pub fn suffix(&self) -> &'static str {
        match self {
            Compression::None => "",
            Compression::Gzip => ".gz",
            Compression::Xz => ".xz",
            Compression::Zstd => ".zst",
        }
    }

    /// Decompress `input` and write the raw tar bytes to `output`.
    pub fn decompress(&self, mut input: impl Read, output: &mut impl Write) -> Result<()> {
        match self {
            Compression::None => {
                std::io::copy(&mut input, output)?;
            }
            Compression::Gzip => {
                let mut dec = flate2::read::GzDecoder::new(input);
                std::io::copy(&mut dec, output)?;
            }
            Compression::Xz => {
                let mut dec = xz2::read::XzDecoder::new(input);
                std::io::copy(&mut dec, output)?;
            }
            Compression::Zstd => {
                let mut dec = zstd::stream::Decoder::new(input)?;
                std::io::copy(&mut dec, output)?;
            }
        }
        Ok(())
    }

    /// Compress the raw tar bytes from `input` into `output`. Uses
    /// reproducible settings (no embedded mtime in gzip).
    pub fn compress(&self, mut input: impl Read, output: impl Write) -> Result<()> {
        match self {
            Compression::None => {
                let mut sink = output;
                std::io::copy(&mut input, &mut sink)?;
            }
            Compression::Gzip => {
                // mtime(0) suppresses the timestamp so output is reproducible.
                let mut enc = flate2::GzBuilder::new()
                    .mtime(0)
                    .write(output, flate2::Compression::default());
                std::io::copy(&mut input, &mut enc)?;
                enc.finish()?;
            }
            Compression::Xz => {
                let mut enc = xz2::write::XzEncoder::new(output, 6);
                std::io::copy(&mut input, &mut enc)?;
                enc.finish()?;
            }
            Compression::Zstd => {
                let mut enc = zstd::stream::Encoder::new(output, 3)?;
                std::io::copy(&mut input, &mut enc)?;
                enc.finish()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn from_filename_matches_extensions() {
        assert_eq!(
            Compression::from_filename("data.tar").unwrap(),
            Compression::None
        );
        assert_eq!(
            Compression::from_filename("data.tar.gz").unwrap(),
            Compression::Gzip
        );
        assert_eq!(
            Compression::from_filename("control.tar.xz").unwrap(),
            Compression::Xz
        );
        assert_eq!(
            Compression::from_filename("data.tar.zst").unwrap(),
            Compression::Zstd
        );
        assert!(Compression::from_filename("data.tar.bz2").is_err());
    }

    #[test]
    fn roundtrip_each_compression() {
        let payload = b"abcdefghij".repeat(100);
        for c in [
            Compression::None,
            Compression::Gzip,
            Compression::Xz,
            Compression::Zstd,
        ] {
            let mut compressed = Vec::new();
            c.compress(Cursor::new(&payload), &mut compressed).unwrap();
            let mut back = Vec::new();
            c.decompress(Cursor::new(&compressed), &mut back).unwrap();
            assert_eq!(back, payload, "{:?} round-trip failed", c);
        }
    }
}
