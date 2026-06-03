use image::{ImageBuffer, Rgba};
use std::io::Cursor;

/// Helper: read a little-endian u32 from `bytes` at `offset`, advancing `offset` by 4.
#[inline(always)]
fn read_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let b: &[u8; 4] = bytes.get(*offset..*offset + 4)?.try_into().ok()?;
    *offset += 4;
    Some(u32::from_le_bytes(*b))
}

/// Helper: skip `n` bytes (used for padding / reserved fields).
#[inline(always)]
fn skip(bytes: &[u8], offset: &mut usize, n: usize) -> Option<()> {
    if *offset + n > bytes.len() { return None; }
    *offset += n;
    Some(())
}

/// Read 8 bytes as a raw array (used for magic/version strings).
#[inline(always)]
fn read_magic(bytes: &[u8], offset: &mut usize) -> Option<[u8; 8]> {
    let b: &[u8; 8] = bytes.get(*offset..*offset + 8)?.try_into().ok()?;
    *offset += 8;
    Some(*b)
}

#[derive(Debug, Clone)]
pub struct Tex {
    pub texv: String,
    pub texi: String,
    pub texb: String,
    pub size: u32,
    pub dimension: [u32; 2],
    pub image_count: u32,
    /// Mipmap chain length advertised by the `.tex` header. `0` for older
    /// `TEXB0001/0002/0003` headers that don't carry a mip count. This is
    /// the **advertised** count.
    pub mipmap_count: u32,
    /// Number of mip levels actually present in the parsed data (level 0 in
    /// `payload` + every entry in `mip_levels`). Always at least 1.
    ///
    /// This is the **actual** count, and it can be lower than
    /// [`Tex::mipmap_count`] when:
    /// - the renderer opts out of CPU mip generation via
    ///   [`Tex::build_mip_chain_with_policy`] with
    ///   [`MipBuildPolicy::GpuGenerate`], or
    /// - the on-disk BCn payload is shorter than the header promises (the
    ///   splitter stops at the last complete level).
    ///
    /// Refreshed automatically by `build_mip_chain*`. Right after
    /// [`Tex::new`] returns it is `1` (only level 0 is in `payload`).
    /// See also [`Tex::missing_mip_count`] and
    /// [`Tex::needs_gpu_mip_generation`].
    pub actual_mip_count: u32,
    pub lz4: bool,
    pub decompressed_size: u32,
    pub extension: String,
    /// Level 0 (full-resolution) payload. For BCn textures this is only
    /// the top-level block data; remaining mip levels are in [`mip_levels`].
    pub payload: Vec<u8>,
    /// Remaining mip levels (level 1..N), each stored as raw bytes.
    /// Empty if no mip chain was built, if the GPU is expected to generate
    /// the rest, or if the on-disk payload didn't contain the full chain.
    pub mip_levels: Vec<Vec<u8>>,
}

impl Tex {
    /// Parse a .tex file from raw bytes using direct byte slicing (no BufReader overhead).
    pub fn new(bytes: &[u8]) -> Option<Tex> {
        let mut pos: usize = 0;

        // Magic/version: TEXV0005 (8 bytes)
        let texv_magic = read_magic(bytes, &mut pos)?;
        let texv = String::from_utf8_lossy(&texv_magic).into_owned();

        // Padding byte
        skip(bytes, &mut pos, 1)?;

        // TEXI0002 (8 bytes)
        let texi_magic = read_magic(bytes, &mut pos)?;
        let texi = String::from_utf8_lossy(&texi_magic).into_owned();

        // Padding byte
        skip(bytes, &mut pos, 1)?;

        // Format (u32)
        let format = read_u32(bytes, &mut pos)?;

        // Reserved (4 bytes)
        skip(bytes, &mut pos, 4)?;

        // Width, Height (u32 × 2)
        let w = read_u32(bytes, &mut pos)?;
        let h = read_u32(bytes, &mut pos)?;

        // 12 bytes reserved/padding
        skip(bytes, &mut pos, 12)?;

        // TEXB0003 or TEXB0004 (8 bytes)
        let texb_magic = read_magic(bytes, &mut pos)?;
        let texb = String::from_utf8_lossy(&texb_magic).into_owned();

        // Padding byte
        skip(bytes, &mut pos, 1)?;

        // Image count (u32)
        let image_count = read_u32(bytes, &mut pos)?;

        // 8 bytes reserved
        skip(bytes, &mut pos, 8)?;

        // Mipmap count (only in TEXB0004)
        let mipmap_count = if texb_magic[7] == b'4' {
            read_u32(bytes, &mut pos)?
        } else {
            0
        };

        // 8 bytes reserved
        skip(bytes, &mut pos, 8)?;

        // LZ4 flag (u32)
        let lz4 = read_u32(bytes, &mut pos)? == 1;

        // Decompressed size (u32)
        let decompressed_size = read_u32(bytes, &mut pos)?;

        // Payload size (u32)
        let payload_size = read_u32(bytes, &mut pos)? as usize;

        // Read payload via direct slice (avoids zero-initialized Vec + read_exact)
        let payload = bytes.get(pos..pos + payload_size)?.to_vec();

        // Determine extension (before LZ4 decompression — raw magic bytes live
        // in the compressed payload; only format==0 has uncompressed embedded images)
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
        let payload = if lz4 {
            lz4_flex::block::decompress(&payload, decompressed_size as usize).ok()?
        } else {
            payload
        };

        Some(Tex {
            texv,
            texi,
            texb,
            size: payload_size as u32,
            dimension: [w, h],
            image_count,
            mipmap_count,
            // Right after parsing only level 0 (in `payload`) is present.
            // `build_mip_chain_with_policy` will refresh this value.
            actual_mip_count: 1,
            lz4,
            decompressed_size,
            payload,
            extension,
            mip_levels: Vec::new(),
        })
    }

    /// Extract the mip levels that are physically present in the payload.
    ///
    /// The parser does **not** synthesize mips on the CPU — mipmap
    /// generation is the renderer's job (the GPU is far faster at it). What
    /// this function does is purely extraction:
    ///
    /// - For BCn compressed textures (`dxt1` / `dxt5`): the engine stores
    ///   the mip chain concatenated in the payload (`level0 | level1 | ...`)
    ///   and the splitter slices it into per-level vectors, putting the
    ///   first slice back into `payload` and the rest into `mip_levels`.
    ///   If the payload is shorter than [`Tex::mipmap_count`] promises
    ///   (common in practice — many `.tex` files ship only level 0 and let
    ///   the runtime generate the rest), the splitter stops at the last
    ///   complete level and the gap is reported via
    ///   [`Tex::missing_mip_count`].
    /// - For every other format (RGBA, R8, RG88, PNG, JPG, MP4, GIF): the
    ///   on-disk payload only contains level 0, so there is nothing to
    ///   extract. `mip_levels` stays empty and the renderer is expected
    ///   to generate the full chain on the GPU at upload time.
    ///
    /// In both cases this function refreshes [`Tex::actual_mip_count`].
    /// Calling it multiple times is safe — `mip_levels` is cleared at the
    /// start so a previous run never leaves stale entries behind.
    pub fn build_mip_chain(&mut self) {
        // Always start from a clean slate so re-running (or running after
        // a different payload was reparsed) doesn't leave stale entries.
        self.mip_levels.clear();

        if self.mipmap_count <= 1 {
            // Either no mips were advertised, or there's only a single level.
            self.actual_mip_count = 1;
            return;
        }

        match self.extension.as_str() {
            "dxt1" | "dxt5" => self.split_bcn_mip_levels(),
            // RGBA / R8 / RG88 / PNG / JPG / etc.: no mip data to extract
            // from the payload. The GPU is expected to generate the chain.
            _ => {}
        }

        // Refresh the actual level count. `split_bcn_mip_levels` may have
        // stopped early (payload too short), so we can't just trust
        // `mipmap_count` here.
        self.actual_mip_count = (self.mip_levels.len() + 1) as u32;
    }

    /// Method-style accessor for [`Tex::actual_mip_count`].
    pub fn actual_mip_count(&self) -> u32 {
        self.actual_mip_count
    }

    /// How many mip levels are still missing relative to the header's
    /// advertised [`Tex::mipmap_count`]. `0` means the full chain is
    /// present (or that the header didn't advertise any mips in the first
    /// place).
    ///
    /// This is the value the renderer should consult to decide whether to
    /// enable GPU-side mip generation for the trailing levels (e.g. via
    /// `wgpu::Features::TEXTURE_FORMAT_*-mipmap-autogen` or by issuing
    /// `queue.write_texture` for each synthesized level).
    pub fn missing_mip_count(&self) -> u32 {
        self.mipmap_count.saturating_sub(self.actual_mip_count)
    }

    /// `true` when at least one advertised mip level is missing from the
    /// parsed data and the renderer will need to generate it on the GPU.
    pub fn needs_gpu_mip_generation(&self) -> bool {
        self.missing_mip_count() > 0
    }

    /// Split BCn payload into individual mip levels.
    /// BCn mip levels are concatenated: level0 + level1 + ... + level(N-1).
    /// Each level's size is determined by its dimensions (halved each level).
    ///
    /// If the payload doesn't contain enough data for all advertised mip
    /// levels (common when the engine stores only level 0 and generates
    /// mips at runtime), we silently stop at the last complete level.
    fn split_bcn_mip_levels(&mut self) {
        let (w, h) = (self.dimension[0] as usize, self.dimension[1] as usize);
        let block_size = if self.extension == "dxt1" { 8 } else { 16 };

        let mut offset = 0usize;
        let mut level_w = w;
        let mut level_h = h;
        let max_levels = self.mipmap_count as usize;
        let mut levels: Vec<Vec<u8>> = Vec::with_capacity(max_levels);

        for _ in 0..max_levels {
            let blocks_w = level_w.div_ceil(4);
            let blocks_h = level_h.div_ceil(4);
            let level_size = blocks_w * blocks_h * block_size;

            if offset + level_size > self.payload.len() {
                break;
            }

            let level_data = self.payload[offset..offset + level_size].to_vec();
            levels.push(level_data);
            offset += level_size;

            level_w = (level_w / 2).max(1);
            level_h = (level_h / 2).max(1);
        }

        if levels.is_empty() {
            return;
        }

        // Level 0 stays in payload; rest go to mip_levels.
        self.payload = levels.remove(0);
        self.mip_levels = levels;
        // Update dimension to level 0 size (may differ from header if header
        // was the full mip-chain size or if some levels were truncated).
        self.dimension = [w as u32, h as u32];
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
            "jpg" | "png" | "mp4" | "gif" => Some((self.payload.clone(), self.extension.clone())),
            _ => None,
        }
    }

    pub fn parse_to_rgba(&mut self) -> Option<()> {
        let (w, h) = (self.dimension[0] as usize, self.dimension[1] as usize);

        match self.extension.as_str() {
            "png" => {
                let img = image::load_from_memory_with_format(&self.payload, image::ImageFormat::Png)
                    .ok()?
                    .into_rgba8();
                let (dw, dh) = img.dimensions();
                self.dimension = [dw, dh];
                self.payload = img.as_raw().to_owned();
            }
            "jpg" => {
                let img =
                    image::load_from_memory_with_format(&self.payload, image::ImageFormat::Jpeg)
                        .ok()?
                        .into_rgba8();
                let (dw, dh) = img.dimensions();
                self.dimension = [dw, dh];
                self.payload = img.as_raw().to_owned();
            }
            "dxt1" => {
                // BCn compressed textures are uploaded directly to the GPU
                // as Bc1RgbaUnormSrgb — no CPU decode needed.
                // Just validate the payload is correctly sized for BC1 blocks.
                let expected = w.div_ceil(4) * h.div_ceil(4) * 8;
                if self.payload.len() != expected {
                    log::warn!("dxt1 size mismatch: got {} bytes, expected {}", self.payload.len(), expected);
                }
            }
            "dxt5" => {
                // BC3: 16 bytes per 4×4 block. Pass through to GPU unchanged.
                let expected = w.div_ceil(4) * h.div_ceil(4) * 16;
                if self.payload.len() != expected {
                    log::warn!("dxt5 size mismatch: got {} bytes, expected {}", self.payload.len(), expected);
                }
            }
            // R8, RG88, mp4, and gif are kept in their native format.
            // They will be uploaded with the correct GPU format (R8Unorm / Rg8Unorm)
            // by the renderer, not expanded to RGBA here.
            "mp4" | "gif" | "rg88" | "r8" => {}
            // Unknown format ("tex") — treat payload as raw RGBA if size matches
            _ => {
                let expected = w * h * 4;
                if self.payload.len() == expected {
                    // Already RGBA, no conversion needed
                } else {
                    return None;
                }
            }
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
    // GIF87a or GIF89a
    if bytes.len() >= 6
        && bytes[..3] == [0x47, 0x49, 0x46]
        && (bytes[3] == 0x38)
        && (bytes[4] == 0x37 || bytes[4] == 0x39)
        && bytes[5] == 0x61
    {
        return "gif";
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
