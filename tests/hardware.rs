//! Runs on a Jetson only; elsewhere every test returns early.

use jetson_multimedia::{ColorFormat, Filter, Jetson, JpegEncoder, Surface, Tag};

fn jetson() -> Option<Jetson> {
    if !std::path::Path::new("/etc/nv_tegra_release").exists() {
        eprintln!("not a Jetson, skipping");
        return None;
    }
    Some(Jetson::load().expect("a Jetson should load its multimedia libraries"))
}

#[test]
fn allocates_a_surface_with_a_dmabuf() {
    let Some(jetson) = jetson() else { return };
    let surface = Surface::new(&jetson, 640, 480, ColorFormat::Uyvy, Tag::Camera).unwrap();
    let info = surface.info();
    assert!(info.fd >= 0);
    assert_eq!((info.width, info.height), (640, 480));
    assert!(info.pitch >= 640 * 2);
    assert_eq!(info.color_format, ColorFormat::Uyvy as u32);
    let borrowed = Surface::from_fd(&jetson, info.fd).unwrap();
    assert_eq!(borrowed.info(), info);
}

#[test]
fn converts_and_encodes_a_decodable_jpeg() {
    let Some(jetson) = jetson() else { return };
    let (width, height) = (1280, 720);
    let camera = Surface::new(&jetson, width, height, ColorFormat::Uyvy, Tag::Camera).unwrap();
    let yuv420 = Surface::new(&jetson, width, height, ColorFormat::Yuv420, Tag::None).unwrap();
    jetson.transform(&camera, &yuv420, Filter::Nearest).unwrap();
    let mut encoder = JpegEncoder::new(&jetson, width, height).unwrap();
    for _ in 0..3 {
        let jpeg = encoder.encode(&yuv420, 85).unwrap().to_vec();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8], "starts with SOI");
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xff, 0xd9], "ends with EOI");
        let mut decoder = jpeg_decoder::Decoder::new(jpeg.as_slice());
        decoder.decode().unwrap();
        let info = decoder.info().unwrap();
        assert_eq!((info.width as u32, info.height as u32), (width, height));
    }
}

#[test]
fn scales_on_the_vic() {
    let Some(jetson) = jetson() else { return };
    let big = Surface::new(&jetson, 1920, 1080, ColorFormat::Uyvy, Tag::Camera).unwrap();
    let small = Surface::new(&jetson, 960, 540, ColorFormat::Yuv420, Tag::VideoConvert).unwrap();
    jetson.transform(&big, &small, Filter::Bilinear).unwrap();
}
