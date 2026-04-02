extern crate alloc;
use alloc::vec::Vec;
use zune_png::PngDecoder;
use zune_png::zune_core::colorspace::ColorSpace;
use zune_jpeg::JpegDecoder;

pub fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes.starts_with(b"\x89PNG") {
        decode_png(bytes)
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        decode_jpg(bytes)
    } else if bytes.starts_with(b"BM") {
        decode_bmp(bytes)
    } else {
        None
    }
}

fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    let mut decoder = PngDecoder::new(bytes);
    decoder.decode_headers().ok()?;
    let (width, height) = decoder.get_dimensions()?;
    let colorspace = decoder.get_colorspace()?;

    let result = decoder.decode().ok()?;
    let raw = match result {
        zune_png::zune_core::result::DecodingResult::U8(v) => v,
        _ => return None,
    };

    let pixels: Vec<u32> = match colorspace {
        ColorSpace::RGB => raw.chunks_exact(3).map(|c| {
            ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
        }).collect(),
        ColorSpace::RGBA => raw.chunks_exact(4).map(|c| {
            ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
        }).collect(),
        ColorSpace::Luma => raw.iter().map(|&l| {
            let l = l as u32;
            (l << 16) | (l << 8) | l
        }).collect(),
        ColorSpace::LumaA => raw.chunks_exact(2).map(|c| {
            let l = c[0] as u32;
            (l << 16) | (l << 8) | l
        }).collect(),
        _ => return None,
    };

    Some((width as u32, height as u32, pixels))
}

fn decode_jpg(bytes: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    let mut decoder = JpegDecoder::new(bytes);
    decoder.decode_headers().ok()?;
    let (width, height) = decoder.dimensions()?;
    let raw = decoder.decode().ok()?;

    // zune-jpeg outputs RGB by default
    let pixels: Vec<u32> = raw.chunks_exact(3).map(|c| {
        ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32)
    }).collect();

    Some((width as u32, height as u32, pixels))
}

fn decode_bmp(bytes: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    use embedded_graphics::pixelcolor::Rgb888;
    use embedded_graphics::prelude::*;
    use tinybmp::Bmp;

    let bmp: Bmp<Rgb888> = Bmp::from_slice(bytes).ok()?;
    let size = bmp.bounding_box().size;
    let width = size.width;
    let height = size.height;

    let mut pixels = alloc::vec![0u32; (width * height) as usize];
    for Pixel(point, color) in bmp.pixels() {
        if point.x >= 0 && point.y >= 0 {
            let idx = point.y as u32 * width + point.x as u32;
            if (idx as usize) < pixels.len() {
                pixels[idx as usize] = ((color.r() as u32) << 16)
                    | ((color.g() as u32) << 8)
                    | (color.b() as u32);
            }
        }
    }

    Some((width, height, pixels))
}
