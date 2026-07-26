use std::{env, path::Path, time::Instant};

use image::GenericImageView;
use webp::{Encoder, WebPConfig};

fn main() -> Result<(), String> {
    let input = env::args()
        .nth(1)
        .ok_or_else(|| "usage: newspaper_webp_bench <image-path>".to_string())?;
    let source = std::fs::read(&input).map_err(|error| error.to_string())?;
    let decode_started = Instant::now();
    let image = image::load_from_memory(&source).map_err(|error| error.to_string())?;
    let dimensions = image.dimensions();
    let rgb = image.to_rgb8();
    let decode_ms = decode_started.elapsed().as_millis();
    let output_dir = Path::new("target").join("newspaper-webp-bench").join(
        Path::new(&input)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("image"),
    );
    std::fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
    println!(
        "input={} dimensions={}x{} bytes={} decode_ms={}",
        Path::new(&input)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image"),
        dimensions.0,
        dimensions.1,
        source.len(),
        decode_ms
    );

    for (label, quality, method, threads) in [
        ("legacy-clear", 92.0, 4, 0),
        ("production-clear", 92.0, 2, 1),
        ("legacy-compact", 45.0, 4, 0),
        ("production-compact", 45.0, 2, 1),
        ("production-very-small", 35.0, 2, 1),
        ("production-maximum-savings", 25.0, 2, 1),
    ] {
        let mut config =
            WebPConfig::new().map_err(|_| "could not initialize WebP config".to_string())?;
        config.quality = quality;
        config.method = method;
        config.thread_level = threads;
        let started = Instant::now();
        let encoded = Encoder::from_rgb(rgb.as_raw(), dimensions.0, dimensions.1)
            .encode_advanced(&config)
            .map_err(|error| format!("{label}: {error:?}"))?;
        let output_path = output_dir.join(format!("{label}.webp"));
        std::fs::write(&output_path, encoded.as_ref()).map_err(|error| error.to_string())?;
        println!(
            "profile={label} quality={quality:.0} method={method} threads={threads} bytes={} ratio={:.3} encode_ms={} output={}",
            encoded.len(),
            encoded.len() as f64 / source.len() as f64,
            started.elapsed().as_millis(),
            output_path.display()
        );
    }
    Ok(())
}
