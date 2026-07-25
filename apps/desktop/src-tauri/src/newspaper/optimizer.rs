use std::path::{Path, PathBuf};

use image::GenericImageView;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptimizationOutcome {
    Replaced { path: PathBuf, bytes: u64 },
    KeptOriginal { bytes: u64 },
}

#[derive(Debug, Error)]
pub enum OptimizationError {
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("optimized image dimensions changed")]
    DimensionMismatch,
    #[error("unsupported optimization profile: {0}")]
    UnsupportedProfile(String),
    #[error("WebP encoder could not accept this image")]
    Encoder,
}

pub fn optimize_page(
    source: &Path,
    profile: &str,
) -> Result<OptimizationOutcome, OptimizationError> {
    let quality = match profile {
        "webp_high" => 92.0,
        "webp_balanced" => 86.0,
        other => return Err(OptimizationError::UnsupportedProfile(other.to_string())),
    };
    let image = image::open(source)?;
    let dimensions = image.dimensions();
    let rgba = image.to_rgba8();
    let encoded =
        webp::Encoder::from_rgba(rgba.as_raw(), dimensions.0, dimensions.1).encode(quality);
    let original_bytes = std::fs::metadata(source)?.len();
    if encoded.len() as u64 >= original_bytes {
        return Ok(OptimizationOutcome::KeptOriginal {
            bytes: original_bytes,
        });
    }

    let output = source.with_extension("webp");
    let part = output.with_extension("webp.part");
    let encoded_bytes: &[u8] = encoded.as_ref();
    std::fs::write(&part, encoded_bytes)?;
    let validated = image::load_from_memory(encoded_bytes)?;
    if validated.dimensions() != dimensions {
        let _ = std::fs::remove_file(&part);
        return Err(OptimizationError::DimensionMismatch);
    }
    if output.exists() {
        std::fs::remove_file(&output)?;
    }
    std::fs::rename(&part, &output)?;
    Ok(OptimizationOutcome::Replaced {
        path: output,
        bytes: encoded.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use tempfile::tempdir;

    #[test]
    fn optimization_preserves_dimensions_and_promotes_valid_webp() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("A01.jpg");
        let image = ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&source, image::ImageFormat::Jpeg)
            .unwrap();

        let outcome = optimize_page(&source, "webp_high").unwrap();
        match outcome {
            OptimizationOutcome::Replaced { path, .. } => {
                assert_eq!(image::open(path).unwrap().dimensions(), (480, 640));
            }
            OptimizationOutcome::KeptOriginal { .. } => {
                assert!(source.exists());
            }
        }
        assert!(!directory.path().join("A01.webp.part").exists());
    }

    #[test]
    fn unsupported_profile_never_changes_the_source() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("A01.jpg");
        std::fs::write(&source, b"source").unwrap();
        assert!(matches!(
            optimize_page(&source, "lossless"),
            Err(OptimizationError::UnsupportedProfile(_))
        ));
        assert_eq!(std::fs::read(source).unwrap(), b"source");
    }
}
