//! Quake 3 shader script (.shader) parser. Every directive is kept as a lowercase-keyword
//! token list so the web viewer can emulate the full multi-stage pipeline.

use crate::vfs::Vfs;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize, Clone, Debug, Default)]
pub struct Shader {
    pub file: String,
    pub params: Vec<Vec<String>>,
    pub stages: Vec<Vec<Vec<String>>>,
}

impl Shader {
    pub fn param(&self, key: &str) -> Option<&Vec<String>> {
        self.params.iter().find(|p| p[0] == key)
    }
    pub fn has_surfaceparm(&self, v: &str) -> bool {
        self.params.iter().any(|p| p[0] == "surfaceparm" && p.get(1).map(|s| s == v).unwrap_or(false))
    }
    /// Image paths referenced by stages (map/clampmap/animmap), ignoring $lightmap etc.
    pub fn stage_images(&self) -> Vec<&str> {
        let mut out = vec![];
        for st in &self.stages {
            for d in st {
                match d[0].as_str() {
                    "map" | "clampmap" if d.len() > 1 && !d[1].starts_with('$') => out.push(d[1].as_str()),
                    "animmap" if d.len() > 2 => out.extend(d[2..].iter().map(|s| s.as_str())),
                    _ => {}
                }
            }
        }
        out
    }
}

pub type ShaderDb = BTreeMap<String, Shader>;

/// Split into lines of tokens, honoring // and /* */ comments and quoted strings.
fn tokenize(text: &str) -> Vec<Vec<String>> {
    let mut lines = vec![];
    let mut cur: Vec<String> = vec![];
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == b'"' {
            let s = i + 1;
            i = s;
            while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                i += 1;
            }
            cur.push(String::from_utf8_lossy(&b[s..i]).into_owned());
            i += 1;
        } else if c == b'{' || c == b'}' {
            // Braces are always their own "line" to simplify block parsing.
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            lines.push(vec![(c as char).to_string()]);
            i += 1;
        } else {
            let s = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'{' && b[i] != b'}' {
                if b[i] == b'/' && matches!(b.get(i + 1), Some(b'/') | Some(b'*')) {
                    break;
                }
                i += 1;
            }
            cur.push(String::from_utf8_lossy(&b[s..i]).into_owned());
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

fn normalize(mut d: Vec<String>) -> Vec<String> {
    // Keywords and paths are case-insensitive in the engine; keep numbers as-is.
    for t in d.iter_mut() {
        *t = t.to_lowercase().replace('\\', "/");
    }
    d
}

pub fn parse(text: &str, file: &str, db: &mut ShaderDb) {
    let lines = tokenize(text);
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if line[0] == "{" || line[0] == "}" {
            i += 1;
            continue;
        }
        let name = line[0].to_lowercase().replace('\\', "/");
        i += 1;
        if i >= lines.len() || lines[i][0] != "{" {
            continue;
        }
        i += 1;
        let mut sh = Shader { file: file.to_string(), ..Default::default() };
        let mut depth = 1;
        let mut stage: Option<Vec<Vec<String>>> = None;
        while i < lines.len() && depth > 0 {
            let l = &lines[i];
            match l[0].as_str() {
                "{" => {
                    depth += 1;
                    if depth == 2 {
                        stage = Some(vec![]);
                    }
                }
                "}" => {
                    if depth == 2
                        && let Some(s) = stage.take()
                    {
                        sh.stages.push(s);
                    }
                    depth -= 1;
                }
                _ => match (&mut stage, depth) {
                    (Some(s), 2) => s.push(normalize(l.clone())),
                    (None, 1) => sh.params.push(normalize(l.clone())),
                    _ => {}
                },
            }
            i += 1;
        }
        db.entry(name).or_insert(sh);
    }
}

pub fn load_all(vfs: &Vfs) -> ShaderDb {
    let mut db = ShaderDb::new();
    // Later paks override earlier ones, so walk scripts with the newest mod first.
    let mut files: Vec<&String> = vfs.with_extension(".shader").collect();
    files.sort_by_key(|f| std::cmp::Reverse(vfs.files[*f].pak));
    for f in files {
        if let Ok(t) = vfs.read_string(f) {
            parse(&t, f, &mut db);
        }
    }
    db
}

/// The image that best represents a shader in tools without shader support.
pub fn base_image(db: &ShaderDb, vfs: &Vfs, name: &str) -> Option<String> {
    if let Some(sh) = db.get(name) {
        for img in sh.stage_images() {
            if let Some(p) = vfs.resolve_image(img) {
                return Some(p);
            }
        }
        if let Some(p) = sh.param("skyparms").and_then(|s| s.get(1)).and_then(|b| vfs.resolve_image(&format!("{b}_ft")))
        {
            return Some(p);
        }
        if let Some(p) = sh.param("qer_editorimage").and_then(|e| e.get(1)).and_then(|p| vfs.resolve_image(p)) {
            return Some(p);
        }
    }
    vfs.resolve_image(name)
}
