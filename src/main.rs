//! The `okconv-q3` command line tool: converts a Quake III install into a folder (with the viewer) and serves it.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ok_q3_converter::{Ctx, Out, bsp, images, media, models, shader, vfs};

#[derive(Parser)]
#[command(
    name = "okconv-q3",
    about = "Convert Quake 3 (id Tech 3) .pk3 content to glTF/PNG/OGG/WebM with an HTML5 viewer"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert a Quake 3 install into an output folder.
    Convert {
        /// Game folder containing baseq3/ (and optionally missionpack/).
        #[arg(long, default_value = "~/Downloads/Games/PC/Quake 3")]
        game: String,
        #[arg(long, default_value = "output")]
        out: PathBuf,
        /// Only export these groups (comma separated): maps,models,players,weapons,textures,sounds,videos,misc,viewer
        #[arg(long)]
        only: Option<String>,
        /// Only maps whose name contains this string (handy for quick tests).
        #[arg(long)]
        map_filter: Option<String>,
    },
    /// Serve an output folder and open the viewer in the browser.
    Serve {
        #[arg(default_value = "output")]
        dir: PathBuf,
        #[arg(long, default_value_t = 8077)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
}

/// Files go into a folder.
struct FsOut(PathBuf);

impl Out for FsOut {
    fn write(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, bytes).with_context(|| format!("writing {}", p.display()))
    }
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}

/// Run `f` over `items` in parallel, logging failures, and collect the successes.
fn run_all(label: &str, items: Vec<String>, f: impl Fn(&str) -> Result<Value> + Sync) -> Vec<Value> {
    let t = Instant::now();
    let mut out: Vec<Value> = items
        .par_iter()
        .filter_map(|p| match f(p) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("  {label} {p}: {e:#}");
                None
            }
        })
        .collect();
    out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    println!("{label:>9}: {}/{} in {:.1}s", out.len(), items.len(), t.elapsed().as_secs_f32());
    out
}

fn convert(game: &Path, out: &Path, only: Option<&str>, map_filter: Option<&str>) -> Result<()> {
    let t0 = Instant::now();
    let want = |g: &str| only.map(|o| o.split(',').any(|x| x.trim() == g)).unwrap_or(true);
    let vfs = vfs::Vfs::open(game)?;
    println!("Loaded {} files from {} paks ({})", vfs.files.len(), vfs.pak_names.len(), vfs.mods.join(", "));
    let shaders = shader::load_all(&vfs);
    println!("  shaders: {}", shaders.len());
    std::fs::create_dir_all(out)?;
    let fs_out = FsOut(out.to_path_buf());
    let ctx = Ctx::new(&vfs, &shaders, &fs_out);

    // Merge with an existing manifest so partial runs (--only) keep the other sections.
    let manifest_path = out.join("manifest.json");
    let mut manifest: Value =
        std::fs::read(&manifest_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}));
    manifest["mods"] = json!(vfs.mods);
    manifest["paks"] = json!(vfs.pak_names);

    let files =
        |pred: &dyn Fn(&str) -> bool| -> Vec<String> { vfs.files.keys().filter(|k| pred(k)).cloned().collect() };
    let is_img = |k: &str| [".tga", ".jpg", ".jpeg", ".png"].iter().any(|e| k.ends_with(e));

    if want("textures") {
        let all = run_all("textures", files(&is_img), |p| images::export_image(&ctx, p));
        let (ui, tex): (Vec<Value>, Vec<Value>) =
            all.into_iter().partition(|v| images::is_ui(v["path"].as_str().unwrap()));
        manifest["textures"] = json!(tex);
        manifest["ui"] = json!(ui);
    }
    // Texture index + shaders are always written; the viewer resolves shader images through it.
    let index: serde_json::Map<String, Value> = vfs
        .files
        .keys()
        .filter(|k| is_img(k))
        .map(|k| (vfs::strip_ext(k).to_string(), json!(vfs.out_path(k, "png"))))
        .collect();
    ctx.write("textures.json", &serde_json::to_vec(&index)?)?;
    ctx.write("shaders.json", &serde_json::to_vec(&shaders)?)?;

    if want("models") {
        manifest["models"] = json!(run_all("models", files(&|k| k.ends_with(".md3")), |p| models::export_md3(&ctx, p)));
    }
    if want("players") {
        let t = Instant::now();
        let mut p: Vec<Value> = models::player_jobs(&ctx)
            .par_iter()
            .filter_map(|(d, skin)| match models::export_player(&ctx, d, skin) {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("  player {d}/{skin}: {e:#}");
                    None
                }
            })
            .collect();
        p.sort_by(|a, b| a["glb"].as_str().cmp(&b["glb"].as_str()));
        println!("  players: {} in {:.1}s", p.len(), t.elapsed().as_secs_f32());
        manifest["players"] = json!(p);
    }
    if want("weapons") {
        let w = models::export_weapons(&ctx);
        println!("  weapons: {}", w.len());
        manifest["weapons"] = json!(w);
    }
    if want("maps") {
        let maps = files(&|k| {
            k.starts_with("maps/") && k.ends_with(".bsp") && map_filter.map(|f| k.contains(f)).unwrap_or(true)
        });
        let mut m = run_all("maps", maps, |p| bsp::export_map(&ctx, p));
        m.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        manifest["maps"] = json!(m);
    }
    if want("sounds") {
        let all = run_all("sounds", files(&|k| k.ends_with(".wav")), |p| media::export_sound(&ctx, p));
        let (music, sfx): (Vec<Value>, Vec<Value>) =
            all.into_iter().partition(|v| v["path"].as_str().unwrap().starts_with("music/"));
        manifest["sounds"] = json!(sfx);
        manifest["music"] = json!(music);
    }
    if want("videos") {
        manifest["videos"] =
            json!(run_all("videos", files(&|k| k.ends_with(".roq")), |p| media::export_video(&ctx, out, p)));
    }
    if want("misc") {
        let handled = [".tga", ".jpg", ".jpeg", ".png", ".wav", ".roq", ".md3", ".bsp"];
        manifest["misc"] =
            json!(run_all("misc", files(&|k| !handled.iter().any(|e| k.ends_with(e))), |p| media::copy_raw(&ctx, p)));
    }
    if want("viewer") {
        write_viewer(out)?;
    }
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    println!("Done in {:.1}s -> {}", t0.elapsed().as_secs_f32(), out.display());
    println!("Preview: cargo run --release -- serve {}", out.display());
    Ok(())
}

macro_rules! viewer_files {
    ($($p:literal),* $(,)?) => { &[$(($p, include_bytes!(concat!("../viewer/", $p)) as &[u8])),*] };
}
const VIEWER: &[(&str, &[u8])] = viewer_files!(
    "index.html",
    "style.css",
    "app.js",
    "q3shader.js",
    "lib/three.module.js",
    "lib/addons/loaders/GLTFLoader.js",
    "lib/addons/utils/BufferGeometryUtils.js",
    "lib/addons/controls/OrbitControls.js",
);

fn write_viewer(out: &Path) -> Result<()> {
    for (p, bytes) in VIEWER {
        let dest = out.join(p);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::write(dest, bytes)?;
    }
    println!("   viewer: {} files", VIEWER.len());
    Ok(())
}

fn content_type(p: &str) -> &'static str {
    match p.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "glb" => "model/gltf-binary",
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "webm" => "video/webm",
        "c" | "h" | "cfg" | "txt" | "shader" | "skin" | "arena" | "bot" | "config" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn serve(dir: &Path, port: u16, open: bool) -> Result<()> {
    let root = dir.canonicalize().with_context(|| format!("{} not found; run convert first", dir.display()))?;
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let url = format!("http://127.0.0.1:{port}/");
    println!("Serving {} at {url} (Ctrl+C to stop)", root.display());
    if open {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    for req in server.incoming_requests() {
        let root = root.clone();
        std::thread::spawn(move || {
            let raw = req.url().split('?').next().unwrap_or("/").to_string();
            let path = percent_decode(raw.trim_start_matches('/'));
            let path = if path.is_empty() { "index.html".to_string() } else { path };
            let full = root.join(&path);
            let ok = full.canonicalize().map(|c| c.starts_with(&root)).unwrap_or(false);
            let hdr = |k: &str, v: &str| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap();
            let file = match (ok, std::fs::File::open(&full)) {
                (true, Ok(f)) if full.is_file() => f,
                _ => {
                    let _ = req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
                    return;
                }
            };
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            let headers = vec![
                hdr("Content-Type", content_type(&path)),
                hdr("Cache-Control", "no-cache"),
                hdr("Accept-Ranges", "bytes"),
            ];
            // Byte ranges let browsers seek and read durations of audio/video.
            let range =
                req.headers().iter().find(|h| h.field.equiv("Range")).and_then(|h| parse_range(h.value.as_str(), len));
            let _ = match range {
                Some((start, end)) => {
                    use std::io::{Read, Seek, SeekFrom};
                    let mut f = file;
                    let _ = f.seek(SeekFrom::Start(start));
                    let n = end - start + 1;
                    let mut headers = headers;
                    headers.push(hdr("Content-Range", &format!("bytes {start}-{end}/{len}")));
                    req.respond(tiny_http::Response::new(206.into(), headers, f.take(n), Some(n as usize), None))
                }
                None => req.respond(tiny_http::Response::new(200.into(), headers, file, Some(len as usize), None)),
            };
        });
    }
    Ok(())
}

/// "bytes=a-b", "bytes=a-" or "bytes=-n" -> inclusive (start, end).
fn parse_range(v: &str, len: u64) -> Option<(u64, u64)> {
    let (a, b) = v.strip_prefix("bytes=")?.split(',').next()?.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => (len.checked_sub(n.parse().ok()?)?, len.checked_sub(1)?),
        (s, "") => (s.parse().ok()?, len.checked_sub(1)?),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len.checked_sub(1)?)),
    };
    (start <= end).then_some((start, end))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Convert { game, out, only, map_filter } => {
            convert(&expand_home(&game), &out, only.as_deref(), map_filter.as_deref())
        }
        Cmd::Serve { dir, port, no_open } => serve(&dir, port, !no_open),
    }
}
