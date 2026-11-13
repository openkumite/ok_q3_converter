//! Layered virtual filesystem over Quake 3 .pk3 archives (baseq3, then mods such as missionpack).

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::sync::Mutex;
use zip::ZipArchive;

/// Largest pk3 entry `Vfs::read` inflates. Quake III files are a few MB at most; deflate reaches about 1000:1,
/// so without a bound a small crafted pk3 could inflate past the memory of the wasm32 app.
const MAX_ENTRY: u64 = 256 << 20;

/// `Send` on native targets (the CLI reads paks from several threads), nothing on wasm32 (a browser `File` is not
/// `Send`).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// A pak's bytes: a file, a buffer, a browser `File` …
pub trait ReadSeek: Read + Seek + MaybeSend {}
impl<T: Read + Seek + MaybeSend> ReadSeek for T {}

type Archive = ZipArchive<Box<dyn ReadSeek>>;

/// Mod directories in load order. Later mods override earlier ones.
pub const MOD_DIRS: &[&str] = &["baseq3", "missionpack"];

#[derive(Clone, Debug)]
pub struct Entry {
    pub mod_name: String,
    pub pak: usize,
    pub index: usize,
}

pub struct Vfs {
    paks: Vec<Mutex<Archive>>,
    pub pak_names: Vec<String>,
    /// lowercase virtual path -> winning entry
    pub files: BTreeMap<String, Entry>,
    pub mods: Vec<String>,
}

impl Vfs {
    /// The paks of a game folder (`baseq3/*.pk3`, then `missionpack/*.pk3`), read from their files as needed.
    #[cfg(feature = "cli")]
    pub fn open(game_dir: &std::path::Path) -> Result<Vfs> {
        use std::path::PathBuf;
        let mut vfs = Vfs::empty();
        let dirs: Vec<PathBuf> = std::fs::read_dir(game_dir)
            .with_context(|| format!("reading game dir {}", game_dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        for mod_name in MOD_DIRS {
            let Some(dir) = dirs
                .iter()
                .find(|d| d.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(mod_name)).unwrap_or(false))
            else {
                continue;
            };
            let mut paks: Vec<(String, Box<dyn ReadSeek>)> = vec![];
            for e in std::fs::read_dir(dir)? {
                let p = e?.path();
                if !p.extension().map(|e| e.eq_ignore_ascii_case("pk3")).unwrap_or(false) {
                    continue;
                }
                let f = std::fs::File::open(&p).with_context(|| format!("opening {}", p.display()))?;
                paks.push((
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    Box::new(std::io::BufReader::new(f)),
                ));
            }
            vfs.add_mod(mod_name, paks).with_context(|| format!("in {}", dir.display()))?;
        }
        anyhow::ensure!(!vfs.files.is_empty(), "no .pk3 files found under {}", game_dir.display());
        Ok(vfs)
    }

    /// One mod's paks, given as (file name, reader); only their zip directories are read here.
    pub fn from_paks(paks: Vec<(String, Box<dyn ReadSeek>)>, mod_name: &str) -> Result<Vfs> {
        let mut vfs = Vfs::empty();
        vfs.add_mod(mod_name, paks)?;
        Ok(vfs)
    }

    fn empty() -> Vfs {
        Vfs { paks: vec![], pak_names: vec![], files: BTreeMap::new(), mods: vec![] }
    }

    fn add_mod(&mut self, mod_name: &str, mut paks: Vec<(String, Box<dyn ReadSeek>)>) -> Result<()> {
        // The engine loads paks in alphabetical order; later ones win.
        paks.sort_by_key(|(name, _)| name.to_lowercase());
        if paks.is_empty() {
            return Ok(());
        }
        self.mods.push(mod_name.to_string());
        for (name, reader) in paks {
            let archive = ZipArchive::new(reader).with_context(|| format!("opening {name}"))?;
            let pak = self.paks.len();
            for index in 0..archive.len() {
                // An entry whose name does not decode is skipped, not the whole pak.
                let Some(Ok(file)) = archive.name_for_index(index) else { continue };
                if file.ends_with('/') {
                    continue;
                }
                let key = file.replace('\\', "/").to_lowercase();
                self.files.insert(key, Entry { mod_name: mod_name.to_string(), pak, index });
            }
            self.pak_names.push(format!("{mod_name}/{name}"));
            self.paks.push(Mutex::new(archive));
        }
        Ok(())
    }

    pub fn exists(&self, path: &str) -> bool {
        self.files.contains_key(&path.to_lowercase())
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let e = self.files.get(&path.to_lowercase()).with_context(|| format!("not in pk3s: {path}"))?;
        let mut archive = self.paks[e.pak].lock().unwrap_or_else(|e| e.into_inner());
        let mut f = archive.by_index(e.index)?;
        // The size is the zip directory's claim: reserve at most 64 MiB up front, `read_to_end` grows to the real
        // size, up to MAX_ENTRY.
        let mut buf = Vec::with_capacity(f.size().min(64 << 20) as usize);
        f.by_ref().take(MAX_ENTRY + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_ENTRY {
            bail!("{path}: inflates past {} MiB", MAX_ENTRY >> 20);
        }
        Ok(buf)
    }

    pub fn read_string(&self, path: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.read(path)?).into_owned())
    }

    pub fn owner(&self, path: &str) -> Option<&str> {
        self.files.get(&path.to_lowercase()).map(|e| e.mod_name.as_str())
    }

    /// Output-root-relative location of an exported asset: `<mod>/<path with new extension>`.
    pub fn out_path(&self, path: &str, new_ext: &str) -> String {
        let m = self.owner(path).unwrap_or("baseq3");
        format!("{}/{}", m, with_ext(&path.to_lowercase(), new_ext))
    }

    pub fn with_extension<'a>(&'a self, ext: &'a str) -> impl Iterator<Item = &'a String> + 'a {
        self.files.keys().filter(move |k| k.ends_with(ext))
    }

    /// Resolve a texture reference the way the engine does: the extension is ignored and
    /// .tga is tried before .jpg.
    pub fn resolve_image(&self, name: &str) -> Option<String> {
        let base = strip_ext(&name.to_lowercase().replace('\\', "/")).to_string();
        for ext in ["tga", "jpg", "png", "jpeg"] {
            let p = format!("{base}.{ext}");
            if self.files.contains_key(&p) {
                return Some(p);
            }
        }
        None
    }
}

pub fn strip_ext(p: &str) -> &str {
    match (p.rfind('.'), p.rfind('/')) {
        (Some(d), Some(s)) if d > s => &p[..d],
        (Some(d), None) => &p[..d],
        _ => p,
    }
}

pub fn with_ext(p: &str, ext: &str) -> String {
    format!("{}.{}", strip_ext(p), ext)
}

/// Relative URI from a file at `from_file` to `to`, both relative to the output root.
pub fn rel_uri(from_file: &str, to: &str) -> String {
    let depth = from_file.matches('/').count();
    format!("{}{}", "../".repeat(depth), to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    /// An entry whose zip64 directory record claims 2^62 bytes reads as its real bytes, not an abort.
    #[test]
    fn huge_claimed_size() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(true);
        z.start_file("a.txt", opts).unwrap();
        z.write_all(b"hello").unwrap();
        let mut d = z.finish().unwrap().into_inner();
        // The central directory record's zip64 extra field (id 1): uncompressed size first.
        let cd = d.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        let name_len = u16::from_le_bytes([d[cd + 28], d[cd + 29]]) as usize;
        let mut x = cd + 46 + name_len;
        while u16::from_le_bytes([d[x], d[x + 1]]) != 1 {
            x += 4 + u16::from_le_bytes([d[x + 2], d[x + 3]]) as usize;
        }
        d[x + 4..x + 12].copy_from_slice(&(1u64 << 62).to_le_bytes());
        let vfs = Vfs::from_paks(vec![("pak0.pk3".into(), Box::new(Cursor::new(d)))], "baseq3").unwrap();
        // zip 9 accepts the record and `size()` returns the claim, so this reaches the 64 MiB cap in `read`.
        assert_eq!(vfs.read("a.txt").unwrap(), b"hello");
    }

    /// An entry inflating past MAX_ENTRY (zeros deflate to about 1/1000) is an error, not memory for all of it.
    #[test]
    fn inflates_past_max_entry() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        z.start_file("big.bsp", zip::write::SimpleFileOptions::default().large_file(true)).unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..(MAX_ENTRY >> 20) + 1 {
            z.write_all(&chunk).unwrap();
        }
        let d = z.finish().unwrap().into_inner();
        let vfs = Vfs::from_paks(vec![("pak0.pk3".into(), Box::new(Cursor::new(d)))], "baseq3").unwrap();
        assert!(vfs.read("big.bsp").is_err());
    }
}
