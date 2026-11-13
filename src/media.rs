//! Audio (WAV -> OGG Vorbis + original WAV), RoQ video -> WebM,
//! and raw copies of everything else.

use crate::Ctx;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::num::{NonZeroU8, NonZeroU32};
use std::path::Path;
use std::process::{Command, Stdio};

fn encode_ogg(wav: &[u8]) -> Result<(Vec<u8>, f32)> {
    let mut r = hound::WavReader::new(std::io::Cursor::new(wav))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
        }
    };
    if samples.is_empty() {
        bail!("empty wav");
    }
    let mut planes = vec![Vec::with_capacity(samples.len() / ch); ch];
    for (i, s) in samples.iter().enumerate() {
        planes[i % ch].push(*s);
    }
    let duration = planes[0].len() as f32 / spec.sample_rate as f32;
    let mut out = vec![];
    let mut enc = vorbis_rs::VorbisEncoderBuilder::new(
        NonZeroU32::new(spec.sample_rate).context("rate")?,
        NonZeroU8::new(ch as u8).context("channels")?,
        &mut out,
    )?
    .build()?;
    for start in (0..planes[0].len()).step_by(4096) {
        let block: Vec<&[f32]> = planes.iter().map(|p| &p[start..(start + 4096).min(p.len())]).collect();
        enc.encode_audio_block(&block)?;
    }
    enc.finish()?;
    Ok((out, duration))
}

pub fn export_sound(ctx: &Ctx, path: &str) -> Result<Value> {
    let data = ctx.vfs.read(path)?;
    let wav = ctx.vfs.out_path(path, "wav");
    ctx.write(&wav, &data)?;
    let (ogg, duration) = match encode_ogg(&data) {
        Ok((bytes, d)) => {
            let p = ctx.vfs.out_path(path, "ogg");
            ctx.write(&p, &bytes)?;
            (Some(p), d)
        }
        Err(e) => {
            eprintln!("  ogg {path}: {e:#} (kept wav)");
            (None, 0.0)
        }
    };
    Ok(json!({"path": path, "wav": wav, "ogg": ogg, "duration": duration}))
}

/// `out_dir`: the output root (ffmpeg writes the file itself).
pub fn export_video(ctx: &Ctx, out_dir: &Path, path: &str) -> Result<Value> {
    let data = ctx.vfs.read(path)?;
    let out = ctx.vfs.out_path(path, "webm");
    let dest = out_dir.join(&out);
    std::fs::create_dir_all(dest.parent().unwrap())?;
    // The RoQ demuxer stops early on a pipe, so go through a temp file.
    let tmp = out_dir.join(".tmp").join(path.replace('/', "_"));
    std::fs::create_dir_all(tmp.parent().unwrap())?;
    std::fs::write(&tmp, &data)?;
    let res = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "fatal", "-y", "-f", "roq", "-i"])
        .arg(&tmp)
        .args([
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "0",
            "-crf",
            "30",
            "-row-mt",
            "1",
            "-deadline",
            "good",
            "-cpu-used",
            "4",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "libopus",
            "-b:a",
            "96k",
        ])
        .arg(&dest)
        .stdin(Stdio::null())
        .output()
        .context("running ffmpeg (is it installed?)");
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir(tmp.parent().unwrap());
    let res = res?;
    if !res.status.success() || !dest.exists() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&res.stderr));
    }
    Ok(json!({"path": path, "url": out}))
}

pub fn copy_raw(ctx: &Ctx, path: &str) -> Result<Value> {
    let data = ctx.vfs.read(path)?;
    let m = ctx.vfs.owner(path).unwrap_or("baseq3");
    let out = format!("{m}/misc/{path}");
    ctx.write(&out, &data)?;
    Ok(json!({"path": path, "url": out, "size": data.len()}))
}
