//! Times VIC convert + NVJPG encode of a 1920x1080 UYVY frame: `cargo run --release --example encode_bench`.

use std::time::Instant;

use jetson_multimedia::{ColorFormat, Filter, Jetson, JpegEncoder, Surface, Tag};

fn main() -> Result<(), jetson_multimedia::Error> {
    let jetson = Jetson::load()?;
    let (width, height) = (1920, 1080);
    let camera = Surface::new(&jetson, width, height, ColorFormat::Uyvy, Tag::Camera)?;
    let yuv420 = Surface::new(&jetson, width, height, ColorFormat::Yuv420, Tag::None)?;
    let mut encoder = JpegEncoder::new(&jetson, width, height)?;
    let frames = 200;
    let start = Instant::now();
    let mut bytes = 0;
    for _ in 0..frames {
        jetson.transform(&camera, &yuv420, Filter::Nearest)?;
        bytes += encoder.encode(&yuv420, 90)?.len();
    }
    let per_frame = start.elapsed().as_secs_f64() / frames as f64;
    println!(
        "{:?}: {:.2} ms/frame ({:.0} fps), {} bytes/frame",
        jetson.release(),
        per_frame * 1e3,
        1.0 / per_frame,
        bytes / frames
    );
    Ok(())
}
