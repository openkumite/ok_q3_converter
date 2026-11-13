//! The maps' runtime data from Quake III Arena's paks (feature `runtime`): a list of steps run one at a
//! time (`Converter::step`, for a caller that draws progress between them, e.g. a browser worker) or all at once
//! (`Converter::run`). Input is the `baseq3` paks as readers (`Paks::open`, the caller opens the files), output any
//! `Sink`; no file system, no external programs. Every file is the same bytes the CLI's `convert` writes at the same
//! path.
//!
//! Layout (paths under the output folder), all listed in `manifest.json` `files` with size and CRC-32:
//!
//! | path | from |
//! |---|---|
//! | `shaders.json` | every `scripts/*.shader`, parsed (the CLI's table) |
//! | `baseq3/maps/<map>.glb`, `<map>_lightmap.png`, `<map>.entities.json` | every `maps/*.bsp` (items placed as meshes) |
//! | `baseq3/<image>.png` | every image a map uses: its materials' textures, the stage images (animMap frames, cloud layers) and skyparms boxes of their shaders, its levelshot; plus every `env/*` and `levelshots/*` image |
//!
//! `manifest.json`: `{game, format, source: "pk3", converter, paks, maps: [the CLI's map entries], skipped: [{path,
//! error}], files: [{path, size, crc32}]}`. A map or image that fails to convert is skipped (listed in `skipped`), as
//! the CLI does; the rest is still written.

pub use crate::error::{Error, Result};
use crate::shader::{self, ShaderDb};
use crate::vfs::{ReadSeek, Vfs, strip_ext};
use crate::{Ctx, Out, bsp, images};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// The data format written (`manifest.json` `format`), for a reader to check.
pub const FORMAT_VERSION: u32 = 1;
/// `manifest.json` `game`.
pub const GAME: &str = "Quake III Arena";
/// The mod folder the paks are read as (the output's top folder).
pub const MOD: &str = "baseq3";
/// The image steps (the images are split into this many slices, for progress).
pub const IMAGE_STEPS: usize = 32;
/// The sky box faces of a `skyparms` box, in Quake III's order.
const SKY_FACES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];

/// Where the converted files go. `put` replaces an existing file of the same path.
pub trait Sink {
    fn put(&mut self, path: &str, data: &[u8]) -> std::result::Result<(), String>;
}

/// A sink that keeps everything in memory (tests).
#[derive(Default)]
pub struct MemorySink {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Sink for MemorySink {
    fn put(&mut self, path: &str, data: &[u8]) -> std::result::Result<(), String> {
        self.files.insert(path.to_string(), data.to_vec());
        Ok(())
    }
}

/// Progress: the step being done and how many are done of how many.
#[derive(Clone, Debug)]
pub struct Progress {
    pub step: String,
    pub done: usize,
    pub total: usize,
}

/// What a finished conversion wrote.
#[derive(Clone, Debug)]
pub struct Summary {
    /// path → (size, CRC-32), manifest.json not included
    pub files: BTreeMap<String, (u64, u32)>,
    /// the maps converted (`q3dm17` …)
    pub maps: Vec<String>,
    /// the paks read, in load order (`baseq3/PAK0.PK3` …)
    pub paks: Vec<String>,
}

/// CRC-32 (IEEE, the zlib one) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

fn is_image(path: &str) -> bool {
    [".tga", ".jpg", ".jpeg", ".png"].iter().any(|e| path.ends_with(e))
}

/// The `baseq3` paks: their zip directories, read once (cheap: what a setup page shows before converting).
pub struct Paks {
    vfs: Vfs,
}

impl Paks {
    /// The paks as (file name, reader); the load order is their names' (case-insensitive), later ones win. `NoMaps`
    /// when none of them holds a `maps/*.bsp`.
    pub fn open(paks: Vec<(String, Box<dyn ReadSeek>)>) -> Result<Paks> {
        let vfs = Vfs::from_paks(paks, MOD)?;
        let p = Paks { vfs };
        if p.maps().is_empty() {
            return Err(Error::NoMaps);
        }
        Ok(p)
    }

    fn map_paths(&self) -> Vec<String> {
        self.vfs.files.keys().filter(|k| k.starts_with("maps/") && k.ends_with(".bsp")).cloned().collect()
    }

    /// The maps' names, sorted (`q3dm17` …).
    pub fn maps(&self) -> Vec<String> {
        let mut m: Vec<String> = self.map_paths().iter().map(|p| map_name(p).to_string()).collect();
        m.sort();
        m.dedup();
        m
    }

    /// The paks, in load order (`baseq3/PAK0.PK3` …).
    pub fn names(&self) -> &[String] {
        &self.vfs.pak_names
    }
}

fn map_name(path: &str) -> &str {
    strip_ext(path.rsplit('/').next().unwrap_or(path))
}

#[derive(Clone, Debug)]
enum Task {
    Shaders,
    /// the pk3 path of a `.bsp`
    Map(String),
    /// slice k of the images
    Images(usize),
    Finish,
}

impl Task {
    fn name(&self) -> String {
        match self {
            Task::Shaders => "shader scripts".into(),
            Task::Map(p) => format!("map {}", map_name(p)),
            Task::Images(k) => format!("images {}/{IMAGE_STEPS}", k + 1),
            Task::Finish => "manifest".into(),
        }
    }
}

/// The files of one export, kept until it succeeded.
#[derive(Default)]
struct Collect(Mutex<Vec<(String, Vec<u8>)>>);

impl Out for Collect {
    fn write(&self, rel: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push((rel.to_string(), bytes.to_vec()));
        Ok(())
    }
}

/// The conversion of the `baseq3` paks.
pub struct Converter {
    vfs: Vfs,
    shaders: ShaderDb,
    tasks: Vec<Task>,
    next: usize,
    written: BTreeMap<String, (u64, u32)>,
    /// the pk3 images to convert (gathered by the map steps)
    used: BTreeSet<String>,
    /// the images' slices, fixed at the first image step
    images: Vec<String>,
    /// manifest pieces: the maps' entries, what was skipped
    maps: Vec<Value>,
    skipped: Vec<Value>,
}

impl Converter {
    pub fn new(paks: Paks) -> Converter {
        let mut tasks = vec![Task::Shaders];
        let mut maps = paks.map_paths();
        maps.sort_by(|a, b| map_name(a).cmp(map_name(b)).then(a.cmp(b)));
        tasks.extend(maps.into_iter().map(Task::Map));
        tasks.extend((0..IMAGE_STEPS).map(Task::Images));
        tasks.push(Task::Finish);
        Converter {
            vfs: paks.vfs,
            shaders: ShaderDb::new(),
            tasks,
            next: 0,
            written: BTreeMap::new(),
            used: BTreeSet::new(),
            images: Vec::new(),
            maps: Vec::new(),
            skipped: Vec::new(),
        }
    }

    pub fn total(&self) -> usize {
        self.tasks.len()
    }

    pub fn progress(&self) -> Progress {
        Progress {
            step: self.tasks.get(self.next).map(Task::name).unwrap_or_else(|| "done".into()),
            done: self.next,
            total: self.tasks.len(),
        }
    }

    fn put(&mut self, sink: &mut dyn Sink, path: &str, data: &[u8]) -> Result<()> {
        sink.put(path, data).map_err(Error::Sink)?;
        if path != "manifest.json" {
            self.written.insert(path.to_string(), (data.len() as u64, crc32(data)));
        }
        Ok(())
    }

    /// Runs the next step; true when the conversion is complete. After an error the same step runs again.
    pub fn step(&mut self, sink: &mut dyn Sink) -> Result<bool> {
        let Some(task) = self.tasks.get(self.next).cloned() else { return Ok(true) };
        self.run_task(&task, sink).map_err(|e| match e {
            Error::Format(m) => Error::Format(format!("{}: {m}", task.name())),
            e => e,
        })?;
        self.next += 1;
        Ok(self.next == self.tasks.len())
    }

    /// Runs every step; `progress` returns false to cancel (`Error::Cancelled`).
    pub fn run(mut self, sink: &mut dyn Sink, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<Summary> {
        loop {
            if !progress(&self.progress()) {
                return Err(Error::Cancelled);
            }
            if self.step(sink)? {
                return Ok(self.summary());
            }
        }
    }

    pub fn summary(&self) -> Summary {
        Summary {
            files: self.written.clone(),
            maps: self.maps.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect(),
            paks: self.vfs.pak_names.clone(),
        }
    }

    fn run_task(&mut self, task: &Task, sink: &mut dyn Sink) -> Result<()> {
        match task {
            Task::Shaders => {
                self.shaders = shader::load_all(&self.vfs);
                let data = serde_json::to_vec(&self.shaders)?;
                self.put(sink, "shaders.json", &data)?;
            }
            Task::Map(path) => self.map(sink, path)?,
            Task::Images(k) => self.images(sink, *k)?,
            Task::Finish => self.finish(sink)?,
        }
        Ok(())
    }

    /// One map (`bsp::export_map`); its images go into `used`.
    fn map(&mut self, sink: &mut dyn Sink, path: &str) -> Result<()> {
        let out = Collect::default();
        let ctx = Ctx::new(&self.vfs, &self.shaders, &out);
        let entry = match bsp::export_map(&ctx, path) {
            Ok(v) => v,
            Err(e) => {
                self.skipped.push(json!({"path": path, "error": format!("{e:#}")}));
                return Ok(());
            }
        };
        let mut used = std::mem::take(&mut *ctx.used.lock().unwrap_or_else(|e| e.into_inner()));
        let shaders = std::mem::take(&mut *ctx.used_shaders.lock().unwrap_or_else(|e| e.into_inner()));
        for key in &shaders {
            used.extend(self.shader_images(key));
        }
        used.extend(self.vfs.resolve_image(&format!("levelshots/{}", map_name(path))));
        self.used.extend(used);
        for (rel, data) in out.0.into_inner().unwrap_or_else(|e| e.into_inner()) {
            self.put(sink, &rel, &data)?;
        }
        self.maps.push(entry);
        Ok(())
    }

    /// The pk3 images a shader may draw: the image of its name (no script), its stages' images (animMap frames, cloud
    /// layers), its skyparms far and near boxes.
    fn shader_images(&self, key: &str) -> Vec<String> {
        let mut names: Vec<String> = vec![key.to_string()];
        if let Some(sh) = self.shaders.get(key) {
            names.extend(sh.stage_images().into_iter().map(str::to_string));
            if let Some(p) = sh.param("skyparms") {
                // skyparms <farbox> <cloudheight> <nearbox>; "-" is none
                for b in [p.get(1), p.get(3)].into_iter().flatten().filter(|b| b.as_str() != "-") {
                    names.extend(SKY_FACES.iter().map(|f| format!("{b}_{f}")));
                }
            }
        }
        names.iter().filter_map(|n| self.vfs.resolve_image(n)).collect()
    }

    /// Slice `k` of the images (`images::export_image`): `used`, every `env/*` and `levelshots/*` image.
    fn images(&mut self, sink: &mut dyn Sink, k: usize) -> Result<()> {
        if k == 0 {
            let extra = self
                .vfs
                .files
                .keys()
                .filter(|p| (p.starts_with("env/") || p.starts_with("levelshots/")) && is_image(p))
                .cloned()
                .collect::<Vec<_>>();
            self.used.extend(extra);
            self.images = self.used.iter().cloned().collect();
        }
        let n = self.images.len();
        let slice = self.images[k * n / IMAGE_STEPS..(k + 1) * n / IMAGE_STEPS].to_vec();
        // Listed once the slice's writes succeeded: a step retried after a `Sink` error lists them once.
        let mut skipped = vec![];
        for path in slice {
            let out = Collect::default();
            let ctx = Ctx::new(&self.vfs, &self.shaders, &out);
            if let Err(e) = images::export_image(&ctx, &path) {
                skipped.push(json!({"path": path, "error": format!("{e:#}")}));
                continue;
            }
            for (rel, data) in out.0.into_inner().unwrap_or_else(|e| e.into_inner()) {
                self.put(sink, &rel, &data)?;
            }
        }
        self.skipped.extend(skipped);
        Ok(())
    }

    /// manifest.json: the index and every file written with its size and CRC-32.
    fn finish(&mut self, sink: &mut dyn Sink) -> Result<()> {
        let files: Vec<Value> =
            self.written.iter().map(|(p, (s, c))| json!({"path": p, "size": s, "crc32": c})).collect();
        let m = json!({
            "game": GAME,
            "format": FORMAT_VERSION,
            "source": "pk3",
            "converter": format!("ok_q3_converter {}", env!("CARGO_PKG_VERSION")),
            "paks": self.vfs.pak_names,
            "maps": self.maps,
            "skipped": self.skipped,
            "files": files,
        });
        let text = serde_json::to_string_pretty(&m)?;
        self.put(sink, "manifest.json", text.as_bytes())
    }
}
