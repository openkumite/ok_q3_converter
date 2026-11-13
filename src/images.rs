//! Textures (TGA/JPG -> PNG).

use crate::Ctx;
use anyhow::{Context, Result};
use serde_json::{Value, json};

const UI_PREFIXES: &[&str] = &["gfx/", "menu/", "icons/", "ui/", "levelshots/", "fonts/", "textures/sfx/logo"];

pub fn is_ui(path: &str) -> bool {
    UI_PREFIXES.iter().any(|p| path.starts_with(p))
}

pub fn export_image(ctx: &Ctx, path: &str) -> Result<Value> {
    let data = ctx.vfs.read(path)?;
    let fmt = if path.ends_with(".tga") { image::ImageFormat::Tga } else { image::ImageFormat::from_path(path)? };
    let img = image::load_from_memory_with_format(&data, fmt).with_context(|| path.to_string())?;
    let has_alpha = img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let img = if has_alpha {
        image::DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        image::DynamicImage::ImageRgb8(img.to_rgb8())
    };
    let out = ctx.vfs.out_path(path, "png");
    let mut buf = std::io::Cursor::new(vec![]);
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    ctx.write(&out, buf.get_ref())?;
    Ok(json!({"path": path, "url": out, "w": img.width(), "h": img.height(), "alpha": has_alpha}))
}
