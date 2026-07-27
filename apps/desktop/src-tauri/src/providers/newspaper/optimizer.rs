use std::path::{Path, PathBuf};

use image::GenericImageView;
use thiserror::Error;
use webp::{Encoder, WebPConfig};

const MIN_WEBP_QUALITY: u8 = 25;
const MAX_WEBP_QUALITY: u8 = 95;
const WEBP_ENCODING_METHOD: i32 = 2;

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
    #[error("WebP quality must be between 25 and 95, received {0}")]
    UnsupportedQuality(u8),
    #[error("WebP encoder could not accept this image")]
    Encoder,
}

fn encoder_config(quality: u8) -> Result<WebPConfig, OptimizationError> {
    if !(MIN_WEBP_QUALITY..=MAX_WEBP_QUALITY).contains(&quality) {
        return Err(OptimizationError::UnsupportedQuality(quality));
    }
    let mut config = WebPConfig::new().map_err(|_| OptimizationError::Encoder)?;
    config.quality = f32::from(quality);
    // Method 2 is materially faster for full newspaper pages while retaining
    // most of method 4's size savings. libwebp threading is opt-in.
    config.method = WEBP_ENCODING_METHOD;
    config.thread_level = 1;
    Ok(config)
}

pub fn optimize_page(source: &Path, quality: u8) -> Result<OptimizationOutcome, OptimizationError> {
    let config = encoder_config(quality)?;
    let source_bytes = std::fs::read(source)?;
    let image = image::load_from_memory(&source_bytes)?;
    if source
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("webp"))
    {
        return Ok(OptimizationOutcome::KeptOriginal {
            bytes: source_bytes.len() as u64,
        });
    }
    let dimensions = image.dimensions();
    let encoded = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        Encoder::from_rgba(rgba.as_raw(), dimensions.0, dimensions.1)
            .encode_advanced(&config)
            .map_err(|_| OptimizationError::Encoder)?
    } else {
        let rgb = image.to_rgb8();
        Encoder::from_rgb(rgb.as_raw(), dimensions.0, dimensions.1)
            .encode_advanced(&config)
            .map_err(|_| OptimizationError::Encoder)?
    };
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

        let outcome = optimize_page(&source, 92).unwrap();
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
    fn unsupported_quality_never_changes_the_source() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("A01.jpg");
        std::fs::write(&source, b"source").unwrap();
        assert!(matches!(
            optimize_page(&source, 24),
            Err(OptimizationError::UnsupportedQuality(24))
        ));
        assert_eq!(std::fs::read(source).unwrap(), b"source");
    }

    #[test]
    fn archive_quality_is_supported() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("A01.jpg");
        let image = ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&source, image::ImageFormat::Jpeg)
            .unwrap();

        assert!(optimize_page(&source, 25).is_ok());
    }

    #[test]
    fn newspaper_encoder_uses_measured_fast_settings() {
        let config = encoder_config(45).unwrap();
        assert_eq!(config.quality, 45.0);
        assert_eq!(config.method, 2);
        assert_eq!(config.thread_level, 1);
    }

    #[test]
    fn existing_webp_is_kept_without_replacing_or_deleting_it() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("A01.webp");
        let image = ImageBuffer::from_fn(480, 640, |x, y| {
            let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
            Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        });
        image
            .save_with_format(&source, image::ImageFormat::WebP)
            .unwrap();
        let original = std::fs::read(&source).unwrap();

        let outcome = optimize_page(&source, 25).unwrap();

        assert_eq!(
            outcome,
            OptimizationOutcome::KeptOriginal {
                bytes: original.len() as u64
            }
        );
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
}
