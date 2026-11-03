//! The YUV 4:2:0 planes of a decoded frame and their conversion to RGBA.
//!
//! The video decoders hand out planar YUV 4:2:0 pixels: a luma plane at
//! the full resolution and two chroma planes at half the width and half
//! the height.  Turning them into RGB is a matrix and a chroma
//! up-sampling, which the GPU does much faster than the CPU - that is why
//! the frames keep their native format as long as possible.
//!
//! The functions of this module are the CPU fallback: they are used by the
//! callers that need real RGBA pixels (the PNG export of the command line
//! tool) and they are the reference the GPU conversion is checked against.

use std::sync::Arc;

use crate::pkg_parser::video_parser::info::{VideoError, YuvColorspace};

/// Copy one plane of a decoded frame into a tightly packed buffer.
///
/// The decoder may hand out planes whose rows are padded: the `stride` is
/// the number of bytes between the beginning of two rows, which can be
/// larger than the width of the plane.  The rows are copied one by one
/// here, so that the caller always gets a tightly packed plane.
///
/// Returns `None` when the data is too short for the given geometry.
pub fn plane_from_rows(
    data: &[u8],
    stride: usize,
    width: usize,
    height: usize,
) -> Option<Arc<[u8]>> {
    if stride < width {
        return None;
    }

    if height == 0 {
        return Some(Arc::from(Vec::new()));
    }

    let size = width.checked_mul(height)?;

    // The last row of the plane starts at `(height - 1) * stride` and is
    // `width` bytes long: the data has to hold all of it.
    let needed = (height - 1).checked_mul(stride)?.checked_add(width)?;
    if data.len() < needed {
        return None;
    }

    if stride == width {
        // The rows are packed: the whole plane is copied at once.
        return Some(Arc::from(&data[..size]));
    }

    // The rows are padded: they are copied one by one into a buffer that
    // is then shared.
    let mut buffer = Vec::with_capacity(size);
    for row in data.chunks(stride).take(height) {
        buffer.extend_from_slice(&row[..width]);
    }
    Some(Arc::from(buffer))
}

/// Convert the YUV 4:2:0 planes of a frame into tightly packed RGBA8.
///
/// The chroma planes are up-sampled with the nearest neighbour (every
/// 2×2 block of luma pixels shares one chroma sample), which is what the
/// `swscale` conversion of an unscaled frame does as well.
///
/// The values are the gamma encoded (display) RGB: the same values the
/// `swscale` conversion produces.
pub fn yuv420_to_rgba(
    width: u32,
    height: u32,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    colorspace: YuvColorspace,
) -> Result<Vec<u8>, VideoError> {
    let width = width as usize;
    let height = height as usize;
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);

    let luma_size = width.saturating_mul(height);
    let chroma_size = chroma_width.saturating_mul(chroma_height);
    if y.len() < luma_size || u.len() < chroma_size || v.len() < chroma_size {
        return Err(VideoError::Decode(
            "the YUV planes of the frame are too small".to_string(),
        ));
    }

    let coefficients = colorspace.coefficients();
    let mut pixels = vec![0u8; luma_size.saturating_mul(4)];

    for row in 0..height {
        let luma_row = &y[row * width..(row + 1) * width];
        let chroma_row = (row / 2) * chroma_width;
        for column in 0..width {
            let luma = luma_row[column] as f32;
            let blue = u[chroma_row + column / 2] as f32;
            let red = v[chroma_row + column / 2] as f32;

            let rgb = coefficients.convert(luma, blue, red);
            let index = (row * width + column) * 4;
            pixels[index] = to_byte(rgb[0]);
            pixels[index + 1] = to_byte(rgb[1]);
            pixels[index + 2] = to_byte(rgb[2]);
            pixels[index + 3] = 255;
        }
    }

    Ok(pixels)
}

/// Clamp a converted value into the 0..255 range.
///
/// The conversion truncates instead of rounding: the `swscale` conversion
/// does the same, and the two agree within one or two units.
fn to_byte(value: f32) -> u8 {
    if value <= 0.0 {
        0
    } else if value >= 255.0 {
        255
    } else {
        value as u8
    }
}
