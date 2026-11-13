//! MD3 -> glTF: generic models, combined player characters (with named animation clips)
//! and combined weapons (barrel + muzzle flash attached via tags).

use crate::Ctx;
use crate::gltf::{Gltf, q2g, q2g_dir, q2g_rot};
use crate::md3::{self, Md3, Surface};
use crate::shader;
use crate::vfs::{strip_ext, with_ext};
use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use serde_json::{Value, json};
use std::collections::HashMap;

pub fn shader_key(name: &str) -> String {
    strip_ext(&name.to_lowercase().replace('\\', "/")).to_string()
}

/// Material for a Quake shader. `lightmap` is the texture index of the lightmap atlas
/// (bound as occlusion on TEXCOORD_1 so DCC tools keep it around).
pub fn material(g: &mut Gltf, ctx: &Ctx, glb_path: &str, shader_name: &str, lightmap: Option<usize>) -> usize {
    let key = format!("{}|{}", shader_key(shader_name), lightmap.is_some());
    g.material(&key, |g| {
        let sk = shader_key(shader_name);
        ctx.note_shader(&sk);
        let sh = ctx.shaders.get(&sk);
        let mut m = json!({
            "name": sk,
            "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0},
            "extras": {"q3shader": sk, "lightmap": lightmap.is_some()},
        });
        if let Some(img) = shader::base_image(ctx.shaders, ctx.vfs, &sk) {
            let uri = ctx.image_uri(glb_path, &img);
            let t = g.texture(&uri);
            m["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": t});
        }
        if let Some(lm) = lightmap {
            m["occlusionTexture"] = json!({"index": lm, "texCoord": 1});
        }
        if let Some(sh) = sh {
            if let Some(c) = sh.param("cull")
                && matches!(c.get(1).map(|s| s.as_str()), Some("none" | "disable" | "twosided"))
            {
                m["doubleSided"] = json!(true);
            }
            if let Some(first) = sh.stages.first() {
                if first.iter().any(|d| d[0] == "blendfunc") {
                    m["alphaMode"] = json!("BLEND");
                } else if first.iter().any(|d| d[0] == "alphafunc") {
                    m["alphaMode"] = json!("MASK");
                }
            }
        }
        m
    })
}

fn weights_for(frame: usize, targets: usize) -> Vec<f32> {
    let mut w = vec![0.0; targets];
    if frame > 0 && frame <= targets {
        w[frame - 1] = 1.0;
    }
    w
}

/// One glTF mesh for an MD3. Frame 0 is the base shape; frames 1.. become morph targets.
/// `shader_of` returns None to hide a surface (e.g. skins that map it to nothing).
#[allow(clippy::too_many_arguments)]
pub fn md3_mesh(
    g: &mut Gltf,
    ctx: &Ctx,
    glb_path: &str,
    m: &Md3,
    name: &str,
    shader_of: &dyn Fn(&Surface) -> Option<String>,
    animated: bool,
    frame: usize,
) -> Option<usize> {
    let mut prims = vec![];
    let targets = if animated { m.num_frames.saturating_sub(1) } else { 0 };
    for s in &m.surfaces {
        let Some(sh) = shader_of(s) else { continue };
        if s.tris.is_empty() || s.frames.is_empty() {
            continue;
        }
        let base_f = if animated { 0 } else { frame.min(s.frames.len() - 1) };
        let pos: Vec<Vec3> = s.frames[base_f].iter().map(|v| q2g(v.0)).collect();
        let nrm: Vec<Vec3> = s.frames[base_f].iter().map(|v| q2g_dir(v.1)).collect();
        let uv: Vec<f32> = s.st.iter().flat_map(|t| [t[0], t[1]]).collect();
        // Quake front faces are clockwise; glTF wants counter-clockwise.
        let idx: Vec<u32> = s.tris.iter().flat_map(|t| [t[0], t[2], t[1]]).collect();
        let mut attrs = json!({
            "POSITION": g.acc_vec3(&pos, true),
            "NORMAL": g.acc_vec3(&nrm, false),
            "TEXCOORD_0": g.acc_f32(&uv, "VEC2", false, Some(crate::gltf::ARRAY_BUFFER)),
        });
        let mat = material(g, ctx, glb_path, &sh, None);
        let mut prim = json!({"attributes": attrs.take(), "indices": g.acc_indices(&idx), "material": mat});
        if targets > 0 {
            let mut tl = vec![];
            for f in 1..=targets {
                let fr = &s.frames[f.min(s.frames.len() - 1)];
                let dp: Vec<Vec3> = fr.iter().zip(&s.frames[0]).map(|(a, b)| q2g(a.0) - q2g(b.0)).collect();
                let dn: Vec<Vec3> = fr.iter().zip(&s.frames[0]).map(|(a, b)| q2g_dir(a.1) - q2g_dir(b.1)).collect();
                tl.push(json!({"POSITION": g.acc_vec3(&dp, true), "NORMAL": g.acc_vec3(&dn, false)}));
            }
            prim["targets"] = json!(tl);
        }
        prims.push(prim);
    }
    if prims.is_empty() {
        return None;
    }
    let mut mesh = json!({"name": name, "primitives": prims});
    if targets > 0 {
        mesh["weights"] = json!(weights_for(frame, targets));
    }
    Some(g.mesh(mesh))
}

struct Channel {
    node: usize,
    path: &'static str,
    times: Vec<f32>,
    values: Vec<f32>,
    ty: &'static str,
}

fn add_animation(g: &mut Gltf, name: &str, channels: Vec<Channel>, extras: Value) {
    let mut samplers = vec![];
    let mut chans = vec![];
    for c in channels {
        let input = g.acc_f32(&c.times, "SCALAR", true, None);
        let output = g.acc_f32(&c.values, c.ty, false, None);
        samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
        chans.push(json!({"sampler": samplers.len() - 1, "target": {"node": c.node, "path": c.path}}));
    }
    if !chans.is_empty() {
        g.animations.push(json!({"name": name, "channels": chans, "samplers": samplers, "extras": extras}));
    }
}

/// Frame sequence -> times at `fps`. Looping clips get the first frame repeated at the end.
fn frame_seq(first: usize, num: usize, looping: bool, fps: f32) -> (Vec<usize>, Vec<f32>) {
    let mut frames: Vec<usize> = (first..first + num.max(1)).collect();
    if looping && num > 1 {
        frames.push(first);
    }
    let times = (0..frames.len()).map(|i| i as f32 / fps.max(1.0)).collect();
    (frames, times)
}

fn weight_channel(node: usize, m: &Md3, frames: &[usize], times: &[f32]) -> Option<Channel> {
    let t = m.num_frames.saturating_sub(1);
    if t == 0 {
        return None;
    }
    let values = frames.iter().flat_map(|&f| weights_for(f.min(t), t)).collect();
    Some(Channel { node, path: "weights", times: times.to_vec(), values, ty: "SCALAR" })
}

fn tag_channels(node: usize, m: &Md3, tag: &str, frames: &[usize], times: &[f32]) -> Vec<Channel> {
    let mut tr = vec![];
    let mut rot = vec![];
    for &f in frames {
        let Some(t) = m.tag(f, tag) else { return vec![] };
        tr.extend(q2g(t.origin).to_array());
        rot.extend(q2g_rot(t.axis).to_array());
    }
    vec![
        Channel { node, path: "translation", times: times.to_vec(), values: tr, ty: "VEC3" },
        Channel { node, path: "rotation", times: times.to_vec(), values: rot, ty: "VEC4" },
    ]
}

fn tag_node(g: &mut Gltf, m: &Md3, tag: &str, frame: usize) -> usize {
    let (t, r) = m.tag(frame, tag).map(|t| (q2g(t.origin), q2g_rot(t.axis))).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
    g.node(json!({"name": tag, "translation": t.to_array(), "rotation": r.to_array()}))
}

fn write_glb(ctx: &Ctx, path: &str, g: &Gltf) -> Result<()> {
    ctx.write(path, &g.to_glb())
}

/// Every .md3 as its own .glb (multi-frame models get a "frames" clip at 10 fps).
pub fn export_md3(ctx: &Ctx, path: &str) -> Result<Value> {
    let m = md3::parse(&ctx.vfs.read(path)?).with_context(|| path.to_string())?;
    let out = ctx.vfs.out_path(path, "glb");
    let mut g = Gltf::new();
    let name = strip_ext(path.rsplit('/').next().unwrap()).to_string();
    let animated = m.num_frames > 1;
    let mesh =
        md3_mesh(&mut g, ctx, &out, &m, &name, &|s| Some(s.shaders.first().cloned().unwrap_or_default()), animated, 0);
    let mut root = json!({"name": name});
    if let Some(mesh) = mesh {
        root["mesh"] = json!(mesh);
    }
    let root = g.node(root);
    g.roots.push(root);
    let mut tag_nodes = vec![];
    for t in m.tag_names() {
        let n = tag_node(&mut g, &m, &t, 0);
        g.add_child(root, n);
        tag_nodes.push((n, t));
    }
    if animated {
        let (frames, times) = frame_seq(0, m.num_frames, true, 10.0);
        let mut ch: Vec<Channel> = weight_channel(root, &m, &frames, &times).into_iter().collect();
        for (n, t) in &tag_nodes {
            ch.extend(tag_channels(*n, &m, t, &frames, &times));
        }
        add_animation(&mut g, "frames", ch, json!({"loop": true}));
    }
    write_glb(ctx, &out, &g)?;
    let lod = path.ends_with("_1.md3") || path.ends_with("_2.md3");
    Ok(json!({
        "path": path, "glb": out, "frames": m.num_frames, "lod": lod,
        "surfaces": m.surfaces.len(), "tags": m.tag_names(),
        "category": category(path),
    }))
}

fn category(path: &str) -> &'static str {
    let p = path.trim_start_matches("models/");
    match p.split('/').next().unwrap_or("") {
        "players" => "players",
        "weapons" | "weapons2" => "weapons",
        "powerups" => "items",
        "mapobjects" => "map objects",
        "ammo" | "weaphits" | "gibs" | "misc" => "effects & projectiles",
        "flags" | "flag2" => "ctf",
        _ => "other",
    }
}

fn parse_skin(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(',') {
            let (k, v) = (k.trim().to_lowercase(), v.trim().to_lowercase().replace('\\', "/"));
            if !k.starts_with("tag_") {
                m.insert(k, v);
            }
        }
    }
    m
}

const ANIM_NAMES: &[&str] = &[
    "BOTH_DEATH1",
    "BOTH_DEAD1",
    "BOTH_DEATH2",
    "BOTH_DEAD2",
    "BOTH_DEATH3",
    "BOTH_DEAD3",
    "TORSO_GESTURE",
    "TORSO_ATTACK",
    "TORSO_ATTACK2",
    "TORSO_DROP",
    "TORSO_RAISE",
    "TORSO_STAND",
    "TORSO_STAND2",
    "LEGS_WALKCR",
    "LEGS_WALK",
    "LEGS_RUN",
    "LEGS_BACK",
    "LEGS_SWIM",
    "LEGS_JUMP",
    "LEGS_LAND",
    "LEGS_JUMPB",
    "LEGS_LANDB",
    "LEGS_IDLE",
    "LEGS_IDLECR",
    "LEGS_TURN",
    "TORSO_GETFLAG",
    "TORSO_GUARDBASE",
    "TORSO_PATROL",
    "TORSO_FOLLOWME",
    "TORSO_AFFIRMATIVE",
    "TORSO_NEGATIVE",
];
const LEGS_WALKCR: usize = 13;
const LEGS_TURN: usize = 24;
const TORSO_GESTURE: usize = 6;

struct Anim {
    name: &'static str,
    first: usize,
    num: usize,
    looping: bool,
    fps: f32,
}

fn parse_anims(text: &str) -> Vec<Anim> {
    let mut nums: Vec<[f32; 4]> = vec![];
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 4
            && let (Ok(a), Ok(b), Ok(c), Ok(d)) = (t[0].parse(), t[1].parse(), t[2].parse(), t[3].parse())
        {
            nums.push([a, b, c, d]);
        }
    }
    let mut anims: Vec<Anim> = nums
        .iter()
        .zip(ANIM_NAMES)
        .map(|(n, name)| Anim {
            name,
            first: n[0].max(0.0) as usize,
            num: n[1].max(1.0) as usize,
            looping: n[2] > 0.0,
            fps: if n[3] > 0.0 { n[3] } else { 15.0 },
        })
        .collect();
    if anims.len() > LEGS_TURN {
        let skip = anims[LEGS_WALKCR].first.saturating_sub(anims[TORSO_GESTURE].first);
        for a in &mut anims[LEGS_WALKCR..=LEGS_TURN] {
            a.first = a.first.saturating_sub(skip);
        }
    }
    anims
}

/// The player characters to export, (model, skin): one .glb each (`export_player`).
pub fn player_jobs(ctx: &Ctx) -> Vec<(String, String)> {
    let dirs: Vec<String> = ctx
        .vfs
        .files
        .keys()
        .filter_map(|k| k.strip_prefix("models/players/")?.strip_suffix("/lower.md3").map(|s| s.to_string()))
        .filter(|s| !s.contains('/'))
        .collect();
    dirs.iter()
        .flat_map(|d| {
            let prefix = format!("models/players/{d}/lower_");
            ctx.vfs
                .files
                .keys()
                .filter_map(move |k| k.strip_prefix(&prefix)?.strip_suffix(".skin").map(|s| (d.clone(), s.to_string())))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A combined, tag-attached player character.
pub fn export_player(ctx: &Ctx, name: &str, skin: &str) -> Result<Value> {
    let base = format!("models/players/{name}");
    let load = |p: &str| -> Result<Md3> { md3::parse(&ctx.vfs.read(&format!("{base}/{p}.md3"))?) };
    let skinf = |p: &str| parse_skin(&ctx.vfs.read_string(&format!("{base}/{p}_{skin}.skin")).unwrap_or_default());
    let (lower, upper, head) = (load("lower")?, load("upper")?, load("head")?);
    let (sl, su, sh) = (skinf("lower"), skinf("upper"), skinf("head"));
    let anims = parse_anims(&ctx.vfs.read_string(&format!("{base}/animation.cfg")).unwrap_or_default());
    let anim = |n: &str| anims.iter().find(|a| a.name == n);
    let legs_idle = anim("LEGS_IDLE").map(|a| a.first).unwrap_or(0);
    let torso_stand = anim("TORSO_STAND").map(|a| a.first).unwrap_or(0);

    let mod_name = ctx.vfs.owner(&format!("{base}/lower.md3")).unwrap_or("baseq3");
    let out = format!("{mod_name}/players/{name}_{skin}.glb");
    let mut g = Gltf::new();
    fn skin_shader(m: &HashMap<String, String>) -> impl Fn(&Surface) -> Option<String> + '_ {
        move |s: &Surface| match m.get(&s.name) {
            Some(v) if v.is_empty() || v.ends_with("/null") || v.ends_with("null.tga") => None,
            Some(v) => Some(v.clone()),
            None => s.shaders.first().cloned(),
        }
    }
    let lower_mesh = md3_mesh(&mut g, ctx, &out, &lower, "lower", &skin_shader(&sl), true, legs_idle);
    let upper_mesh = md3_mesh(&mut g, ctx, &out, &upper, "upper", &skin_shader(&su), true, torso_stand);
    let head_mesh = md3_mesh(&mut g, ctx, &out, &head, "head", &skin_shader(&sh), false, 0);

    let root = g.node(json!({"name": format!("{name}_{skin}")}));
    g.roots.push(root);
    let mk = |g: &mut Gltf, n: &str, mesh: Option<usize>| {
        let mut v = json!({"name": n});
        if let Some(m) = mesh {
            v["mesh"] = json!(m);
        }
        g.node(v)
    };
    let n_lower = mk(&mut g, "lower", lower_mesh);
    g.add_child(root, n_lower);
    let n_torso = tag_node(&mut g, &lower, "tag_torso", legs_idle);
    g.add_child(n_lower, n_torso);
    let n_upper = mk(&mut g, "upper", upper_mesh);
    g.add_child(n_torso, n_upper);
    let n_headtag = tag_node(&mut g, &upper, "tag_head", torso_stand);
    g.add_child(n_upper, n_headtag);
    let n_head = mk(&mut g, "head", head_mesh);
    g.add_child(n_headtag, n_head);
    let n_weapon = tag_node(&mut g, &upper, "tag_weapon", torso_stand);
    g.add_child(n_upper, n_weapon);

    let mut clips = vec![];
    for a in &anims {
        let (frames, times) = frame_seq(a.first, a.num, a.looping, a.fps);
        let mut ch = vec![];
        let legs = a.name.starts_with("LEGS") || a.name.starts_with("BOTH");
        let torso = a.name.starts_with("TORSO") || a.name.starts_with("BOTH");
        if legs {
            ch.extend(weight_channel(n_lower, &lower, &frames, &times));
            ch.extend(tag_channels(n_torso, &lower, "tag_torso", &frames, &times));
        }
        if torso {
            ch.extend(weight_channel(n_upper, &upper, &frames, &times));
            ch.extend(tag_channels(n_headtag, &upper, "tag_head", &frames, &times));
            ch.extend(tag_channels(n_weapon, &upper, "tag_weapon", &frames, &times));
        }
        let part = if a.name.starts_with("BOTH") {
            "both"
        } else if legs {
            "legs"
        } else {
            "torso"
        };
        add_animation(&mut g, a.name, ch, json!({"loop": a.looping, "part": part, "fps": a.fps}));
        clips.push(json!({"name": a.name, "part": part, "loop": a.looping}));
    }
    write_glb(ctx, &out, &g)?;
    let icon = ctx.vfs.resolve_image(&format!("{base}/icon_{skin}")).map(|p| ctx.vfs.out_path(&p, "png"));
    Ok(json!({"name": name, "skin": skin, "glb": out, "icon": icon, "clips": clips,
              "sounds": format!("sound/player/{name}/")}))
}

/// Combined weapons: base model + barrel (tag_barrel, with a spin clip) + flash (tag_flash).
pub fn export_weapons(ctx: &Ctx) -> Vec<Value> {
    let mut mains: Vec<String> = ctx
        .vfs
        .files
        .keys()
        .filter(|k| k.starts_with("models/weapons2/") && k.ends_with(".md3"))
        .filter(|k| {
            let stem = strip_ext(k.rsplit('/').next().unwrap());
            let dir = k.split('/').nth(2).unwrap_or("");
            // The main model is the one named after its folder (plus a few oddly named ones).
            !["_barrel", "_flash", "_hand", "_1", "_2", "_3"].iter().any(|s| stem.ends_with(s))
                && (stem == dir || ctx.vfs.exists(&k.replace(".md3", "_hand.md3")))
        })
        .cloned()
        .collect();
    mains.sort();
    mains.dedup();
    mains
        .iter()
        .filter_map(|p| match export_weapon(ctx, p) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("  weapon {p}: {e:#}");
                None
            }
        })
        .collect()
}

fn export_weapon(ctx: &Ctx, path: &str) -> Result<Value> {
    let name = strip_ext(path.rsplit('/').next().unwrap()).to_string();
    let mod_name = ctx.vfs.owner(path).unwrap_or("baseq3");
    let out = format!("{mod_name}/weapons/{name}.glb");
    let main = md3::parse(&ctx.vfs.read(path)?)?;
    let mut g = Gltf::new();
    let sh = |s: &Surface| Some(s.shaders.first().cloned().unwrap_or_default());
    let mesh = md3_mesh(&mut g, ctx, &out, &main, &name, &sh, false, 0);
    let mut rv = json!({"name": name});
    if let Some(m) = mesh {
        rv["mesh"] = json!(m);
    }
    let root = g.node(rv);
    g.roots.push(root);
    let mut parts = vec![];
    for (suffix, tag) in [("barrel", "tag_barrel"), ("flash", "tag_flash")] {
        let p = path.replace(".md3", &format!("_{suffix}.md3"));
        let Ok(data) = ctx.vfs.read(&p) else { continue };
        let Ok(m) = md3::parse(&data) else { continue };
        let tn = tag_node(&mut g, &main, tag, 0);
        g.add_child(root, tn);
        let mesh = md3_mesh(&mut g, ctx, &out, &m, suffix, &sh, false, 0);
        let mut v = json!({"name": suffix});
        if let Some(mesh) = mesh {
            v["mesh"] = json!(mesh);
        }
        let n = g.node(v);
        g.add_child(tn, n);
        parts.push(suffix);
        if suffix == "barrel" {
            // Barrels roll around the tag's forward (X) axis.
            let times: Vec<f32> = (0..=4).map(|i| i as f32 * 0.1).collect();
            let values: Vec<f32> = (0..=4)
                .flat_map(|i| Quat::from_rotation_x(i as f32 * std::f32::consts::FRAC_PI_2).to_array())
                .collect();
            add_animation(
                &mut g,
                "barrel_spin",
                vec![Channel { node: n, path: "rotation", times, values, ty: "VEC4" }],
                json!({"loop": true}),
            );
        }
    }
    let hand = path.replace(".md3", "_hand.md3");
    write_glb(ctx, &out, &g)?;
    let icon = ctx
        .vfs
        .resolve_image(&format!("icons/iconw_{name}"))
        .or_else(|| {
            let alt = match name.as_str() {
                "rocketl" => "rocket",
                "grenadel" => "grenade",
                "machinegun" => "machinegun",
                n => n,
            };
            ctx.vfs.resolve_image(&format!("icons/iconw_{alt}"))
        })
        .map(|p| ctx.vfs.out_path(&p, "png"));
    Ok(json!({"name": name, "glb": out, "parts": parts, "icon": icon,
              "hand": ctx.vfs.exists(&hand).then(|| ctx.vfs.out_path(&hand, "glb")),
              "sounds": "sound/weapons/"}))
}

/// Static (single-frame) MD3 geometry for embedding in other files, e.g. items in maps.
pub fn md3_static(g: &mut Gltf, ctx: &Ctx, glb_path: &str, path: &str) -> Option<usize> {
    let m = md3::parse(&ctx.vfs.read(path).ok()?).ok()?;
    let name = with_ext(path.rsplit('/').next().unwrap(), "");
    md3_mesh(
        g,
        ctx,
        glb_path,
        &m,
        name.trim_end_matches('.'),
        &|s| Some(s.shaders.first().cloned().unwrap_or_default()),
        false,
        0,
    )
}
