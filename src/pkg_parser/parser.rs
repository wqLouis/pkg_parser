use std::{
    collections::HashMap,
    fs::{self, File, create_dir_all},
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use crate::pkg_parser::tex_parser;

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
    pub fn new(pkg_path: &Path) -> Pkg {
        let mut file = BufReader::new(File::open(pkg_path).unwrap());
        let header = Self::read_header(&mut file);
        let entries = Self::read_entries(&mut file, header.file_count);
        let files = Self::read_files(&mut file, &entries);

        Pkg { header, files }
    }

    fn read_header(file: &mut BufReader<File>) -> Header {
        let mut buf = [0u8; 4];

        // Version string
        file.read_exact(&mut buf).unwrap();
        let version_len = u32::from_le_bytes(buf) as usize;
        let mut version_bytes = vec![0u8; version_len];
        file.read_exact(&mut version_bytes).unwrap();
        let version = String::from_utf8(version_bytes).unwrap();

        // File count
        file.read_exact(&mut buf).unwrap();
        let file_count = u32::from_le_bytes(buf);

        Header { version, file_count }
    }

    fn read_entries(file: &mut BufReader<File>, entry_count: u32) -> Vec<Entry> {
        let mut entries = Vec::with_capacity(entry_count as usize);
        let mut buf = [0u8; 4];

        for _ in 0..entry_count {
            // Path
            file.read_exact(&mut buf).unwrap();
            let path_len = u32::from_le_bytes(buf) as usize;
            let mut path_bytes = vec![0u8; path_len];
            file.read_exact(&mut path_bytes).unwrap();

            // Offset and size
            file.read_exact(&mut buf).unwrap();
            let offset = u32::from_le_bytes(buf);
            file.read_exact(&mut buf).unwrap();
            let size = u32::from_le_bytes(buf);

            entries.push(Entry {
                path: String::from_utf8(path_bytes).unwrap(),
                offset,
                size,
            });
        }

        entries
    }

    fn read_files(file: &mut BufReader<File>, entries: &[Entry]) -> HashMap<String, Vec<u8>> {
        let data_start = file.stream_position().unwrap();
        let mut map = HashMap::with_capacity(entries.len());

        // Sort by offset to read sequentially and avoid random seeking
        let mut sorted: Vec<&Entry> = entries.iter().collect();
        sorted.sort_by_key(|e| e.offset);

        for entry in &sorted {
            file.seek(SeekFrom::Start(entry.offset as u64 + data_start))
                .unwrap();
            let mut buf = vec![0u8; entry.size as usize];
            file.read_exact(&mut buf).unwrap();
            map.insert(entry.path.clone(), buf);
        }

        map
    }

    pub fn save_pkg(&self, target: &Path, dry_run: bool, parse_tex: bool, verbose: bool) {
        for (path, bytes) in &self.files {
            let output_path = target.join(path);

            if parse_tex && Path::new(path).extension().unwrap_or_default() == "tex" {
                let Some(tex) = tex_parser::Tex::new(bytes) else {
                    println!("failed to parse tex: {}", path);
                    continue;
                };

                if verbose {
                    println!("Texture:");
                    println!("Texv: {}", tex.texv);
                    println!("Texi: {}", tex.texi);
                    println!("Texb: {}", tex.texb);
                    println!("Image count: {}", tex.image_count);
                    println!("Mipmap count: {}", tex.mipmap_count);
                    println!("Lz4 compressed: {}", tex.lz4);
                    println!("Texture size: {}", tex.size);
                    println!("w: {} h: {}", tex.dimension[0], tex.dimension[1]);
                    println!();
                }

                let Some((img_data, img_ext)) = tex.parse_to_image() else {
                    println!("failed to parse image: {}", path);
                    continue;
                };

                let mut img_path = output_path;
                img_path.set_extension(&img_ext);

                if !dry_run {
                    create_dir_all(img_path.parent().unwrap()).unwrap();
                    fs::write(&img_path, &img_data).unwrap();
                }
            } else {
                if !dry_run {
                    create_dir_all(output_path.parent().unwrap()).unwrap();
                    fs::write(&output_path, bytes).unwrap();
                }
            }
        }
    }
}
