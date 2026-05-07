use std::io::{BufReader, Cursor, Read};

use image::{ImageBuffer, Rgba};

#[derive(Debug, Clone)]
pub struct Tex {
    pub texv: String,
    pub texi: String,
    pub texb: String,
    pub size: u32,
    pub dimension: [u32; 2],
    pub image_count: u32,
    pub mipmap_count: u32,
    pub lz4: bool,
    pub decompressed_size: u32,
    pub extension: String,
    pub payload: Vec<u8>,
}

impl Tex {
    pub fn new(bytes: &[u8]) -> Option<Tex> {
        let mut buf = BufReader::new(Cursor::new(bytes));

        // Read magic/version strings
        let mut magic = [0u8; 8];
        buf.read_exact(&mut magic).ok()?;
        let texv = String::from_utf8_lossy(&magic).into_owned();

        buf.seek_relative(1).ok()?;
        buf.read_exact(&mut magic).ok()?;
        let texi = String::from_utf8_lossy(&magic).into_owned();

        // Format and dimensions
        buf.seek_relative(1).ok()?;
        let mut u32_buf = [0u8; 4];
        buf.read_exact(&mut u32_buf).ok()?;
        let format = u32::from_le_bytes(u32_buf);

        buf.seek_relative(4).ok()?;
        buf.read_exact(&mut u32_buf).ok()?;
        let w = u32::from_le_bytes(u32_buf);
        buf.read_exact(&mut u32_buf).ok()?;
        let h = u32::from_le_bytes(u32_buf);

        // Block magic
        buf.seek_relative(12).ok()?;
        buf.read_exact(&mut magic).ok()?;
        let texb = String::from_utf8_lossy(&magic).into_owned();

        // Image and mipmap count
        buf.seek_relative(1).ok()?;
        buf.read_exact(&mut u32_buf).ok()?;
        let image_count = u32::from_le_bytes(u32_buf);

        buf.seek_relative(8).ok()?;
        let mut mipmap_count = 0u32;
        if texb == "TEXB0004" {
            buf.read_exact(&mut u32_buf).ok()?;
            mipmap_count = u32::from_le_bytes(u32_buf);
        }

        // LZ4 flag and sizes
        buf.seek_relative(8).ok()?;
        buf.read_exact(&mut u32_buf).ok()?;
        let lz4 = u32::from_le_bytes(u32_buf) == 1;
        buf.read_exact(&mut u32_buf).ok()?;
        let decompressed_size = u32::from_le_bytes(u32_buf);
        buf.read_exact(&mut u32_buf).ok()?;
        let payload_size = u32::from_le_bytes(u32_buf);

        // Payload
        let mut payload = vec![0u8; payload_size as usize];
        buf.read_exact(&mut payload).ok()?;

        // Determine extension (before LZ4 decompression — raw magic bytes live in the compressed payload)
        let extension = match format {
            0 => match_signature(&payload),
            7 => "dxt1",
            4 | 6 => "dxt5",
            8 => "rg88",
            9 => "r8",
            _ => "tex",
        };
        let extension = extension.to_owned();

        // Decompress if LZ4
        if lz4 {
            payload = lz4_flex::block::decompress(&payload, decompressed_size as usize).ok()?;
        }

        Some(Tex {
            texv,
            texi,
            texb,
            size: payload_size,
            dimension: [w, h],
            image_count,
            mipmap_count,
            lz4,
            decompressed_size,
            payload,
            extension,
        })
    }

    pub fn parse_to_image(&self) -> Option<(Vec<u8>, String)> {
        let (w, h) = (self.dimension[0], self.dimension[1]);

        match self.extension.as_str() {
            "r8" => {
                let rgba: Vec<u8> = self.payload.iter().flat_map(|&b| [b, b, b, 255]).collect();
                raw_to_png(rgba, w, h)
            }
            "rg88" => {
                let rgba: Vec<u8> = self
                    .payload
                    .windows(2)
                    .flat_map(|b| [b[0], b[0], b[0], b[1]])
                    .collect();
                raw_to_png(rgba, w, h)
            }
            "dxt1" => {
                let decoded = bcndecode::decode(
                    &self.payload,
                    w as usize,
                    h as usize,
                    bcndecode::BcnEncoding::Bc1,
                    bcndecode::BcnDecoderFormat::RGBA,
                )
                .ok()?;
                raw_to_png(decoded, w, h)
            }
            "dxt5" => {
                let decoded = bcndecode::decode(
                    &self.payload,
                    w as usize,
                    h as usize,
                    bcndecode::BcnEncoding::Bc3,
                    bcndecode::BcnDecoderFormat::RGBA,
                )
                .ok()?;
                raw_to_png(decoded, w, h)
            }
            "jpg" | "png" | "mp4" => Some((self.payload.clone(), self.extension.clone())),
            _ => None,
        }
    }

    pub fn parse_to_rgba(&mut self) -> Option<()> {
        let (w, h) = (self.dimension[0] as usize, self.dimension[1] as usize);

        self.payload = match self.extension.as_str() {
            "png" => image::load_from_memory_with_format(&self.payload, image::ImageFormat::Png)
                .ok()?
                .into_rgba8()
                .as_raw()
                .to_owned(),
            "jpg" => image::load_from_memory_with_format(&self.payload, image::ImageFormat::Jpeg)
                .ok()?
                .into_rgba8()
                .as_raw()
                .to_owned(),
            "dxt1" => bcndecode::decode(
                &self.payload,
                w,
                h,
                bcndecode::BcnEncoding::Bc1,
                bcndecode::BcnDecoderFormat::RGBA,
            )
            .ok()?,
            "dxt5" => bcndecode::decode(
                &self.payload,
                w,
                h,
                bcndecode::BcnEncoding::Bc3,
                bcndecode::BcnDecoderFormat::RGBA,
            )
            .ok()?,
            // R8, RG88, and mp4 are kept in their native format.
            // They will be uploaded with the correct GPU format (R8Unorm / Rg8Unorm)
            // by the renderer, not expanded to RGBA here.
            "mp4" | "rg88" | "r8" => self.payload.clone(),
            _ => return None,
        };

        Some(())
    }
}

/// Detect embedded image format from magic bytes.
fn match_signature(bytes: &[u8]) -> &str {
    if bytes.len() >= 8 && bytes[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return "png";
    }
    if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        return "jpg";
    }
    if bytes.len() >= 8 && bytes[4..8] == [0x66, 0x74, 0x79, 0x70] {
        return "mp4";
    }
    "tex"
}

fn raw_to_png(bytes: Vec<u8>, w: u32, h: u32) -> Option<(Vec<u8>, String)> {
    let mut buf = Vec::new();
    let mut cur = Cursor::new(&mut buf);
    ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, bytes)?
        .write_to(&mut cur, image::ImageFormat::Png)
        .ok()?;
    Some((buf, "png".to_owned()))
}
