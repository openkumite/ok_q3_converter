//! Quake III Arena (id Tech 3) asset conversion library: the `.pk3` file system, shader scripts, BSP maps and MD3
//! models to glTF, images to PNG.
//!
//! Features: `cli` (default) the command line tool's folder output, viewer, sounds (Vorbis) and videos (ffmpeg).
//! Without `cli` the library has no file system access and no external programs (it builds for wasm32); the paks
//! come in as `vfs::ReadSeek` readers and files go out through an `Out`. `runtime` adds `runtime::Converter`, the
//! step-by-step conversion of the maps into runtime data, for a program that converts in-app.

pub mod bsp;
#[cfg(feature = "runtime")]
pub mod error;
pub mod gltf;
pub mod images;
pub mod md3;
#[cfg(feature = "cli")]
pub mod media;
pub mod models;
#[cfg(feature = "runtime")]
pub mod runtime;
pub mod shader;
pub mod vfs;

use anyhow::Result;
use std::collections::BTreeSet;
use std::sync::Mutex;

/// Where exported files go, by path relative to the output root (`baseq3/maps/q3dm17.glb`).
pub trait Out: Sync {
    fn write(&self, rel: &str, bytes: &[u8]) -> Result<()>;
}

/// What an export reads and where it writes.
pub struct Ctx<'a> {
    pub vfs: &'a vfs::Vfs,
    pub shaders: &'a shader::ShaderDb,
    pub out: &'a dyn Out,
    /// the pk3 images the exported files name (lowercase pk3 paths)
    pub used: Mutex<BTreeSet<String>>,
    /// the shaders of the materials exported (keys of `shaders`, or image names without a script)
    pub used_shaders: Mutex<BTreeSet<String>>,
}

impl<'a> Ctx<'a> {
    pub fn new(vfs: &'a vfs::Vfs, shaders: &'a shader::ShaderDb, out: &'a dyn Out) -> Ctx<'a> {
        Ctx { vfs, shaders, out, used: Mutex::new(BTreeSet::new()), used_shaders: Mutex::new(BTreeSet::new()) }
    }

    pub fn write(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        self.out.write(rel, bytes)
    }

    /// Records a material's shader in `used_shaders`.
    pub fn note_shader(&self, key: &str) {
        self.used_shaders.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string());
    }

    /// The URI by which the file at `glb_path` names the PNG of the pk3 image `vfs_img`; records `vfs_img` in `used`.
    pub fn image_uri(&self, glb_path: &str, vfs_img: &str) -> String {
        self.used.lock().unwrap_or_else(|e| e.into_inner()).insert(vfs_img.to_string());
        vfs::rel_uri(glb_path, &self.vfs.out_path(vfs_img, "png"))
    }
}
