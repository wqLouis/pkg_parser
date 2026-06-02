use std::{
    collections::HashMap,
    fs::{self, File, create_dir_all},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use crate::pkg_parser::{mdl_parser, tex_parser, video_parser};
use log;

#[derive(Debug, Clone)]
pub struct Pkg {
    pub header: Header,
    pub files: HashMap<String, Vec<u8>>,
}

/// Package file header containing metadata
#[derive(Debug, Clone)]
pub struct Header {
    /// Version string of the package
    pub version: String,
    /// Number of files in the package
    #[allow(dead_code)]
    pub file_count: u32,
}

pub(self) struct Entry {
    pub path: String,
    pub offset: u32,
    pub size: u32,
}

impl Pkg {
    /// Open and parse a `.pkg` file.
    ///
    /// Returns `Ok(Pkg)` on success or `Err(io::Error)` if the file
    /// cannot be read or contains malformed data.
    pub fn new(pkg_path: &Path) -> Result<Pkg, io::Error> {
        let mut file = BufReader::new(File::open(pkg_path)?);
        let header = Self::read_header(&mut file)?;
        let entries = Self::read_entries(&mut file, header.file_count)?;
        let files = Self::read_files(&mut file, &entries)?;

        Ok(Pkg { header, files })
    }

    fn read_header(file: &mut BufReader<File>) -> io::Result<Header> {
        let mut buf = [0u8; 4];

        // Version string
        file.read_exact(&mut buf)?;
        let version_len = u32::from_le_bytes(buf) as usize;
        let mut version_bytes = vec![0u8; version_len];
        file.read_exact(&mut version_bytes)?;
        let version = String::from_utf8(version_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // File count
        file.read_exact(&mut buf)?;
        let file_count = u32::from_le_bytes(buf);

        Ok(Header {
            version,
            file_count,
        })
    }

    fn read_entries(file: &mut BufReader<File>, entry_count: u32) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::with_capacity(entry_count as usize);
        let mut buf = [0u8; 4];

        for _ in 0..entry_count {
            // Path
            file.read_exact(&mut buf)?;
            let path_len = u32::from_le_bytes(buf) as usize;
            let mut path_bytes = vec![0u8; path_len];
            file.read_exact(&mut path_bytes)?;

            // Offset and size
            file.read_exact(&mut buf)?;
            let offset = u32::from_le_bytes(buf);
            file.read_exact(&mut buf)?;
            let size = u32::from_le_bytes(buf);

            entries.push(Entry {
                path: String::from_utf8(path_bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                offset,
                size,
            });
        }

        Ok(entries)
    }

    fn read_files(file: &mut BufReader<File>, entries: &[Entry]) -> io::Result<HashMap<String, Vec<u8>>> {
        let data_start = file.stream_position()?;
        let mut map = HashMap::with_capacity(entries.len());

        // Sort by offset to read sequentially and avoid random seeking
        let mut sorted: Vec<&Entry> = entries.iter().collect();
        sorted.sort_by_key(|e| e.offset);

        for entry in &sorted {
            file.seek(SeekFrom::Start(entry.offset as u64 + data_start))?;
            let mut buf = vec![0u8; entry.size as usize];
            file.read_exact(&mut buf)?;
            map.insert(entry.path.clone(), buf);
        }

        Ok(map)
    }

    pub fn save_pkg(
        &self,
        target: &Path,
        dry_run: bool,
        parse_tex: bool,
        parse_video: bool,
        parse_mdl: bool,
    ) {
        for (path, bytes) in &self.files {
            let output_path = target.join(path);
            let ext = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();

            if parse_tex && ext.eq_ignore_ascii_case("tex") {
                save_tex_file(path, bytes, &output_path, dry_run);
            } else if parse_video && (ext.eq_ignore_ascii_case("mp4") || ext.eq_ignore_ascii_case("webm") || ext.eq_ignore_ascii_case("gif")) {
                save_video_file(path, bytes, &output_path, dry_run);
            } else if parse_mdl && ext.eq_ignore_ascii_case("mdl") {
                save_mdl_file(path, bytes, &output_path, dry_run);
            } else if !dry_run {
                create_dir_all(output_path.parent().unwrap()).unwrap();
                fs::write(&output_path, bytes).unwrap();
            }
        }
    }
}

/// Process and save a .tex texture file.
fn save_tex_file(path: &str, bytes: &[u8], output_path: &Path, dry_run: bool) {
    let Some(tex) = tex_parser::Tex::new(bytes) else {
        log::warn!("failed to parse tex: {}", path);
        return;
    };

    log::info!("Texture: {}", path);
    log::debug!("  Texv: {}", tex.texv);
    log::debug!("  Texi: {}", tex.texi);
    log::debug!("  Texb: {}", tex.texb);
    log::debug!("  Image count: {}", tex.image_count);
    log::debug!("  Mipmap count: {}", tex.mipmap_count);
    log::debug!("  Lz4 compressed: {}", tex.lz4);
    log::debug!("  Texture size: {}", tex.size);
    log::debug!("  w: {} h: {}", tex.dimension[0], tex.dimension[1]);

    let Some((img_data, img_ext)) = tex.parse_to_image() else {
        log::warn!("failed to parse image: {}", path);
        return;
    };

    let mut img_path = output_path.to_path_buf();
    img_path.set_extension(&img_ext);

    if !dry_run {
        create_dir_all(img_path.parent().unwrap()).unwrap();
        fs::write(&img_path, &img_data).unwrap();
    }
}

/// Process and save a video or GIF file.
fn save_video_file(path: &str, bytes: &[u8], output_path: &Path, dry_run: bool) {
    let Some(video) = video_parser::Video::new(bytes) else {
        log::warn!("failed to parse video/gif: {}", path);
        return;
    };

    log::info!("Video/GIF: {}", path);
    log::debug!("  Format: {:?}", video.format);
    if let Some((w, h)) = video.dimensions {
        log::debug!("  Dimensions: {}x{}", w, h);
    }
    if let Some(count) = video.frame_count {
        log::debug!("  Frame count: {}", count);
    }

    if video.is_gif() {
        // Save GIF as-is and also extract frames
        if !dry_run {
            create_dir_all(output_path.parent().unwrap()).unwrap();
            fs::write(output_path, bytes).unwrap();
        }

        let stem = Path::new(path)
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("output");
        if let Some(frames) = video_parser::save_gif_frames(bytes, stem) {
            for (frame_data, suffix) in &frames {
                let frame_path = output_path.with_file_name(suffix);
                if !dry_run {
                    fs::write(&frame_path, frame_data).unwrap();
                }
            }
        }
    } else if !dry_run {
        // Save video/gif file as-is
        create_dir_all(output_path.parent().unwrap()).unwrap();
        fs::write(output_path, bytes).unwrap();
    }
}

/// Process and save a .mdl puppet model file.
fn save_mdl_file(path: &str, bytes: &[u8], output_path: &Path, dry_run: bool) {
    let Some(mdl) = mdl_parser::MdlFile::new(bytes) else {
        log::warn!("failed to parse mdl: {}", path);
        if !dry_run {
            create_dir_all(output_path.parent().unwrap()).unwrap();
            fs::write(output_path, bytes).unwrap();
        }
        return;
    };

    log::info!("Puppet model: {}", path);
    log::debug!(
        "  Records: {}, Quads: {}, Triangles: {}",
        mdl.data.records.len(),
        mdl.data.quads.len(),
        mdl.data.triangles.len()
    );
    log::debug!(
        "  Bones: {}, Frames: {}",
        mdl.bones.bones.len(),
        mdl.animation.num_frames
    );

    // Write raw .mdl file
    if !dry_run {
        create_dir_all(output_path.parent().unwrap()).unwrap();
        fs::write(output_path, bytes).unwrap();
    }

    // Write parsed JSON
    match mdl.to_json() {
        Ok(json) => {
            let mut json_path = output_path.to_path_buf();
            json_path.set_extension("mdl.json");
            if !dry_run {
                fs::write(&json_path, &json).unwrap();
            }
        }
        Err(e) => {
            log::warn!("failed to serialize mdl json: {}: {}", path, e);
        }
    }
}
