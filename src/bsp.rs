//! IBSP v46 (Quake 3) level -> glTF: tessellated patches, lightmap atlas on TEXCOORD_1,
//! vertex colors, brush entities as separate nodes, and item models placed at spawn points.

use crate::Ctx;
use crate::gltf::{ARRAY_BUFFER, Gltf, q2g, q2g_dir};
use crate::models::{material, md3_static, shader_key};
use crate::vfs::{rel_uri, strip_ext};
use anyhow::{Result, bail};
use glam::{Quat, Vec2, Vec3, Vec4};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};

const PATCH_LEVEL: usize = 8;
/// Largest patch control grid side Q3 loads (MAX_PATCH_SIZE; ParseMesh drops the map past it).
const MAX_PATCH_SIZE: usize = 32;
/// Vertices plus indices one map may emit. The largest Quake III Arena map (q3ctf3) emits about 0.76 M; the bound
/// only stops crafted maps whose surfaces all share one vertex range (tiny once deflated) from exhausting memory.
const MAX_EMITTED: usize = 8 << 20;
/// Overbright shift applied to lightmaps/vertex colors, as the engine does without hardware gamma.
const OVERBRIGHT_SHIFT: u32 = 2;

#[derive(Clone, Copy, Default)]
struct Vert {
    pos: Vec3,
    st: Vec2,
    lm: Vec2,
    normal: Vec3,
    color: Vec4,
}

impl Vert {
    fn lerp(a: &Vert, b: &Vert, t: f32) -> Vert {
        Vert {
            pos: a.pos.lerp(b.pos, t),
            st: a.st.lerp(b.st, t),
            lm: a.lm.lerp(b.lm, t),
            normal: a.normal.lerp(b.normal, t),
            color: a.color.lerp(b.color, t),
        }
    }
}

struct Surface {
    shader: usize,
    kind: i32,
    first_vert: usize,
    num_verts: usize,
    first_index: usize,
    num_indexes: usize,
    lightmap: i32,
    patch_w: usize,
    patch_h: usize,
}

struct Bsp {
    entities: String,
    shaders: Vec<(String, i32, i32)>,
    models: Vec<(usize, usize)>,
    verts: Vec<Vert>,
    indexes: Vec<u32>,
    surfaces: Vec<Surface>,
    lightmaps: Vec<Vec<u8>>,
    fogs: Vec<String>,
}

fn i32_at(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn cstr(d: &[u8]) -> String {
    let end = d.iter().position(|&c| c == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).to_lowercase().replace('\\', "/")
}

/// Scale up and clamp while preserving hue, like R_ColorShiftLightingBytes.
fn shift_color(r: u8, g: u8, b: u8) -> [u8; 3] {
    let s = 1u32 << OVERBRIGHT_SHIFT;
    let (mut r, mut g, mut b) = (r as u32 * s, g as u32 * s, b as u32 * s);
    let max = r.max(g).max(b);
    if max > 255 {
        r = r * 255 / max;
        g = g * 255 / max;
        b = b * 255 / max;
    }
    [r as u8, g as u8, b as u8]
}

fn parse(d: &[u8]) -> Result<Bsp> {
    if d.len() < 8 + 17 * 8 || &d[0..4] != b"IBSP" || i32_at(d, 4) != 46 {
        bail!("not an IBSP v46 map");
    }
    let lump = |n: usize| -> Result<&[u8]> {
        let o = i32_at(d, 8 + n * 8) as usize;
        let l = i32_at(d, 12 + n * 8) as usize;
        o.checked_add(l).and_then(|e| d.get(o..e)).ok_or_else(|| anyhow::anyhow!("lump {n} out of range"))
    };
    let entities = cstr(lump(0)?);
    let shaders = lump(1)?.as_chunks::<72>().0.iter().map(|c| (cstr(&c[..64]), i32_at(c, 64), i32_at(c, 68))).collect();
    let models =
        lump(7)?.as_chunks::<40>().0.iter().map(|c| (i32_at(c, 24) as usize, i32_at(c, 28) as usize)).collect();
    let verts = lump(10)?
        .as_chunks::<44>()
        .0
        .iter()
        .map(|c| {
            let f = |o| f32_at(c, o);
            let col = shift_color(c[40], c[41], c[42]);
            Vert {
                pos: Vec3::new(f(0), f(4), f(8)),
                st: Vec2::new(f(12), f(16)),
                lm: Vec2::new(f(20), f(24)),
                normal: Vec3::new(f(28), f(32), f(36)),
                color: Vec4::new(col[0] as f32, col[1] as f32, col[2] as f32, c[43] as f32) / 255.0,
            }
        })
        .collect();
    let indexes = lump(11)?.as_chunks::<4>().0.iter().map(|c| i32_at(c, 0) as u32).collect();
    let fogs = lump(12)?.as_chunks::<72>().0.iter().map(|c| cstr(&c[..64])).collect();
    let surfaces = lump(13)?
        .as_chunks::<104>()
        .0
        .iter()
        .map(|c| {
            let i = |o| i32_at(c, o);
            Surface {
                shader: i(0) as usize,
                kind: i(8),
                first_vert: i(12) as usize,
                num_verts: i(16) as usize,
                first_index: i(20) as usize,
                num_indexes: i(24) as usize,
                lightmap: i(28),
                patch_w: i(96).max(0) as usize,
                patch_h: i(100).max(0) as usize,
            }
        })
        .collect();
    let lightmaps = lump(14)?.as_chunks::<{ 128 * 128 * 3 }>().0.iter().map(|c| c.to_vec()).collect();
    Ok(Bsp { entities, shaders, models, verts, indexes, surfaces, lightmaps, fogs })
}

pub fn parse_entities(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = vec![];
    let mut cur: Option<BTreeMap<String, String>> = None;
    let mut toks = vec![];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' => toks.push(c.to_string()),
            '"' => {
                let s: String = chars.by_ref().take_while(|&c| c != '"').collect();
                toks.push(format!("\"{s}"));
            }
            _ => {}
        }
    }
    let mut i = 0;
    while i < toks.len() {
        match toks[i].as_str() {
            "{" => cur = Some(BTreeMap::new()),
            "}" => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
            }
            k if i + 1 < toks.len() && toks[i + 1].starts_with('"') => {
                if let Some(e) = cur.as_mut() {
                    e.insert(k[1..].to_lowercase(), toks[i + 1][1..].to_string());
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn vec3_of(s: &str) -> Option<Vec3> {
    let v: Vec<f32> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
}

/// Biquadratic Bézier tessellation of a patch control grid.
fn tessellate(ctrl: &[Vert], w: usize, h: usize, verts: &mut Vec<Vert>, idx: &mut Vec<u32>) {
    if w < 3 || h < 3 || w > MAX_PATCH_SIZE || h > MAX_PATCH_SIZE || ctrl.len() < w * h {
        return;
    }
    let l = PATCH_LEVEL;
    for py in (0..h - 1).step_by(2) {
        for px in (0..w - 1).step_by(2) {
            if py + 2 >= h || px + 2 >= w {
                continue;
            }
            let c = |x: usize, y: usize| &ctrl[(py + y) * w + px + x];
            let base = verts.len() as u32;
            for j in 0..=l {
                let v = j as f32 / l as f32;
                // Collapse each row of 3 control points along u, then the column along v.
                let mut col = [Vert::default(); 3];
                for i in 0..=l {
                    let u = i as f32 / l as f32;
                    for (r, item) in col.iter_mut().enumerate() {
                        *item = bez(c(0, r), c(1, r), c(2, r), u);
                    }
                    let mut p = bez(&col[0], &col[1], &col[2], v);
                    p.normal = p.normal.normalize_or(Vec3::Z);
                    verts.push(p);
                }
            }
            let row = (l + 1) as u32;
            for j in 0..l as u32 {
                for i in 0..l as u32 {
                    let a = base + j * row + i;
                    let (b, cc, d) = (a + 1, a + row, a + row + 1);
                    idx.extend([a, b, cc, b, d, cc]);
                }
            }
        }
    }
}

fn bez(a: &Vert, b: &Vert, c: &Vert, t: f32) -> Vert {
    let ab = Vert::lerp(a, b, t);
    let bc = Vert::lerp(b, c, t);
    Vert::lerp(&ab, &bc, t)
}

/// Item classname -> models (from bg_misc.c).
fn item_models(class: &str) -> &'static [&'static str] {
    match class {
        "item_armor_shard" => &["models/powerups/armor/shard.md3", "models/powerups/armor/shard_sphere.md3"],
        "item_armor_combat" => &["models/powerups/armor/armor_yel.md3"],
        "item_armor_body" => &["models/powerups/armor/armor_red.md3"],
        "item_health_small" => &["models/powerups/health/small_cross.md3", "models/powerups/health/small_sphere.md3"],
        "item_health" => &["models/powerups/health/medium_cross.md3", "models/powerups/health/medium_sphere.md3"],
        "item_health_large" => &["models/powerups/health/large_cross.md3", "models/powerups/health/large_sphere.md3"],
        "item_health_mega" => &["models/powerups/health/mega_cross.md3", "models/powerups/health/mega_sphere.md3"],
        "weapon_gauntlet" => &["models/weapons2/gauntlet/gauntlet.md3"],
        "weapon_shotgun" => &["models/weapons2/shotgun/shotgun.md3"],
        "weapon_machinegun" => &["models/weapons2/machinegun/machinegun.md3"],
        "weapon_grenadelauncher" => &["models/weapons2/grenadel/grenadel.md3"],
        "weapon_rocketlauncher" => &["models/weapons2/rocketl/rocketl.md3"],
        "weapon_lightning" => &["models/weapons2/lightning/lightning.md3"],
        "weapon_railgun" => &["models/weapons2/railgun/railgun.md3"],
        "weapon_plasmagun" => &["models/weapons2/plasma/plasma.md3"],
        "weapon_bfg" => &["models/weapons2/bfg/bfg.md3"],
        "weapon_grapplinghook" => &["models/weapons2/grapple/grapple.md3"],
        "weapon_nailgun" => &["models/weapons/nailgun/nailgun.md3"],
        "weapon_prox_launcher" => &["models/weapons/proxmine/proxmine.md3"],
        "weapon_chaingun" => &["models/weapons/vulcan/vulcan.md3"],
        "ammo_shells" => &["models/powerups/ammo/shotgunam.md3"],
        "ammo_bullets" => &["models/powerups/ammo/machinegunam.md3"],
        "ammo_grenades" => &["models/powerups/ammo/grenadeam.md3"],
        "ammo_cells" => &["models/powerups/ammo/plasmaam.md3"],
        "ammo_lightning" => &["models/powerups/ammo/lightningam.md3"],
        "ammo_rockets" => &["models/powerups/ammo/rocketam.md3"],
        "ammo_slugs" => &["models/powerups/ammo/railgunam.md3"],
        "ammo_bfg" => &["models/powerups/ammo/bfgam.md3"],
        "ammo_nails" => &["models/powerups/ammo/nailgunam.md3"],
        "ammo_mines" => &["models/powerups/ammo/proxmineam.md3"],
        "ammo_belt" => &["models/powerups/ammo/chaingunam.md3"],
        "holdable_teleporter" => &["models/powerups/holdable/teleporter.md3"],
        "holdable_medkit" => &["models/powerups/holdable/medkit.md3", "models/powerups/holdable/medkit_sphere.md3"],
        "holdable_kamikaze" => &["models/powerups/kamikazi.md3"],
        "holdable_portal" => &["models/powerups/holdable/porter.md3"],
        "holdable_invulnerability" => &["models/powerups/holdable/invulnerability.md3"],
        "item_quad" => &["models/powerups/instant/quad.md3", "models/powerups/instant/quad_ring.md3"],
        "item_enviro" => &["models/powerups/instant/enviro.md3", "models/powerups/instant/enviro_ring.md3"],
        "item_haste" => &["models/powerups/instant/haste.md3", "models/powerups/instant/haste_ring.md3"],
        "item_invis" => &["models/powerups/instant/invis.md3", "models/powerups/instant/invis_ring.md3"],
        "item_regen" => &["models/powerups/instant/regen.md3", "models/powerups/instant/regen_ring.md3"],
        "item_flight" => &["models/powerups/instant/flight.md3", "models/powerups/instant/flight_ring.md3"],
        "item_scout" => &["models/powerups/scout.md3"],
        "item_guard" => &["models/powerups/guard.md3"],
        "item_doubler" => &["models/powerups/doubler.md3"],
        "item_ammoregen" => &["models/powerups/ammo.md3"],
        "team_ctf_redflag" => &["models/flags/r_flag.md3"],
        "team_ctf_blueflag" => &["models/flags/b_flag.md3"],
        "team_ctf_neutralflag" => &["models/flags/n_flag.md3"],
        _ => &[],
    }
}

/// Why a map that has emitted `emitted` vertices and indices so far stops (None: it goes on). `per_vertex` is the most
/// its own vertices and indices can emit (see the budget comment in `export_map`): more means surfaces share vertex ranges (a crafted map); within
/// it, `MAX_EMITTED` still bounds a map larger than any of the game's.
fn over_budget(emitted: usize, per_vertex: usize) -> Option<String> {
    if emitted > per_vertex {
        Some("map emits more geometry than its vertices hold (surfaces share vertex ranges)".into())
    } else if emitted > MAX_EMITTED {
        Some(format!("map exceeds the converter's geometry limit ({MAX_EMITTED} vertices and indices)"))
    } else {
        None
    }
}

#[derive(Default)]
struct Prim {
    verts: Vec<Vert>,
    idx: Vec<u32>,
}

pub fn export_map(ctx: &Ctx, path: &str) -> Result<Value> {
    let bsp = parse(&ctx.vfs.read(path)?)?;
    let name = strip_ext(path.rsplit('/').next().unwrap()).to_string();
    let mod_name = ctx.vfs.owner(path).unwrap_or("baseq3").to_string();
    let out = format!("{mod_name}/maps/{name}.glb");

    // Lightmap atlas: 128x128 tiles in a square-ish grid.
    let n_lm = bsp.lightmaps.len();
    let cols = (n_lm as f32).sqrt().ceil().max(1.0) as usize;
    let rows = n_lm.div_ceil(cols).max(1);
    let (aw, ah) = (cols * 128, rows * 128);
    let lm_path = format!("{mod_name}/maps/{name}_lightmap.png");
    if n_lm > 0 {
        let mut img = image::RgbImage::new(aw as u32, ah as u32);
        for (i, lm) in bsp.lightmaps.iter().enumerate() {
            let (ox, oy) = ((i % cols) * 128, (i / cols) * 128);
            for y in 0..128 {
                for x in 0..128 {
                    let o = (y * 128 + x) * 3;
                    let c = shift_color(lm[o], lm[o + 1], lm[o + 2]);
                    img.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb(c));
                }
            }
        }
        let mut buf = std::io::Cursor::new(vec![]);
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        ctx.write(&lm_path, buf.get_ref())?;
    }
    let lm_uv = |v: &mut Vert, lmi: i32| {
        if lmi >= 0 && n_lm > 0 {
            let i = lmi as usize;
            let (ox, oy) = ((i % cols) as f32, (i / cols) as f32);
            v.lm = Vec2::new((ox + v.lm.x) * 128.0 / aw as f32, (oy + v.lm.y) * 128.0 / ah as f32);
        }
    };

    let mut g = Gltf::new();
    let lm_tex = (n_lm > 0).then(|| g.texture(&rel_uri(&out, &lm_path)));
    let ents = parse_entities(&bsp.entities);

    let root = g.node(json!({"name": name}));
    g.roots.push(root);
    let mut shaders_used = BTreeMap::<String, usize>::new();

    // Surfaces name their own vertex and index ranges, and nothing stops them from all naming the same one. Real
    // maps copy each draw vertex and index once, and a patch control point tessellates into at most 81/4 vertices
    // and 96 indices; past that (or MAX_EMITTED) the map is an error, not memory growing with the square of its size.
    let per_vertex = bsp.verts.len().saturating_mul(1 + 21 + 96).saturating_add(bsp.indexes.len());
    let mut emitted = 0usize;
    let mut model_node = |g: &mut Gltf, mi: usize, node_name: String| -> Result<Option<usize>> {
        let Some(&(first, count)) = bsp.models.get(mi) else { return Ok(None) };
        let Some(surfaces) = first.checked_add(count).and_then(|e| bsp.surfaces.get(first..e)) else { return Ok(None) };
        let mut prims: BTreeMap<(usize, bool), Prim> = BTreeMap::new();
        let mut spend = |n: usize| -> Result<()> {
            emitted = emitted.saturating_add(n);
            match over_budget(emitted, per_vertex) {
                Some(why) => bail!("{why}"),
                None => Ok(()),
            }
        };
        for s in surfaces {
            let Some((sname, _, _)) = bsp.shaders.get(s.shader) else { continue };
            let sk = shader_key(sname);
            let sh = ctx.shaders.get(&sk);
            if sh.map(|s| s.has_surfaceparm("nodraw")).unwrap_or(false) || s.kind == 4 {
                continue;
            }
            let lit = s.lightmap >= 0;
            let p = prims.entry((s.shader, lit)).or_default();
            let Some(src) = s.first_vert.checked_add(s.num_verts).and_then(|e| bsp.verts.get(s.first_vert..e)) else {
                continue;
            };
            let base = p.verts.len() as u32;
            match s.kind {
                1 | 3 => {
                    // an index past the surface's own vertices (a damaged map; a negative one is huge as u32) would
                    // name another surface's vertex or overflow `base + i`: the surface is left out
                    let ix = s.first_index.checked_add(s.num_indexes).and_then(|e| bsp.indexes.get(s.first_index..e));
                    if ix.is_some_and(|ix| ix.iter().any(|&i| i as usize >= src.len())) {
                        continue;
                    }
                    spend(src.len())?;
                    p.verts.extend_from_slice(src);
                    let Some(ix) = ix else {
                        continue;
                    };
                    spend(ix.len())?;
                    for t in ix.as_chunks::<3>().0 {
                        p.idx.extend([base + t[0], base + t[2], base + t[1]]);
                    }
                }
                2 => {
                    let mut v = vec![];
                    let mut ix = vec![];
                    tessellate(src, s.patch_w, s.patch_h, &mut v, &mut ix);
                    spend(v.len() + ix.len())?;
                    // Make each triangle face along its interpolated normal.
                    for t in ix.as_chunks::<3>().0 {
                        let (a, b, c) = (v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]);
                        let fnorm = (b.pos - a.pos).cross(c.pos - a.pos);
                        let flip = fnorm.dot(a.normal + b.normal + c.normal) < 0.0;
                        let (b1, c1) = if flip { (t[2], t[1]) } else { (t[1], t[2]) };
                        p.idx.extend([base + t[0], base + b1, base + c1]);
                    }
                    p.verts.extend(v);
                }
                _ => continue,
            }
            for v in &mut p.verts[base as usize..] {
                lm_uv(v, s.lightmap);
            }
            *shaders_used.entry(sk).or_default() += 1;
        }
        let mut gprims = vec![];
        for ((si, lit), p) in prims {
            if p.idx.is_empty() {
                continue;
            }
            let pos: Vec<Vec3> = p.verts.iter().map(|v| q2g(v.pos)).collect();
            let nrm: Vec<Vec3> = p.verts.iter().map(|v| q2g_dir(v.normal).normalize_or(Vec3::Y)).collect();
            let st: Vec<f32> = p.verts.iter().flat_map(|v| v.st.to_array()).collect();
            let lm: Vec<f32> = p.verts.iter().flat_map(|v| v.lm.to_array()).collect();
            let col: Vec<f32> = p.verts.iter().flat_map(|v| v.color.to_array()).collect();
            let attrs = json!({
                "POSITION": g.acc_vec3(&pos, true),
                "NORMAL": g.acc_vec3(&nrm, false),
                "TEXCOORD_0": g.acc_f32(&st, "VEC2", false, Some(ARRAY_BUFFER)),
                "TEXCOORD_1": g.acc_f32(&lm, "VEC2", false, Some(ARRAY_BUFFER)),
                "COLOR_0": g.acc_f32(&col, "VEC4", false, Some(ARRAY_BUFFER)),
            });
            let mat = material(g, ctx, &out, &bsp.shaders[si].0, if lit { lm_tex } else { None });
            gprims.push(json!({"attributes": attrs, "indices": g.acc_indices(&p.idx), "material": mat}));
        }
        if gprims.is_empty() {
            return Ok(None);
        }
        let mesh = g.mesh(json!({"name": node_name, "primitives": gprims}));
        Ok(Some(g.node(json!({"name": node_name, "mesh": mesh}))))
    };

    if let Some(n) = model_node(&mut g, 0, "worldspawn".into())? {
        g.add_child(root, n);
    }

    // Entities: brush models, items with embedded models, and markers for everything else.
    let ent_root = g.node(json!({"name": "entities"}));
    g.add_child(root, ent_root);
    let mut item_mesh_cache: HashMap<&str, Option<usize>> = HashMap::new();
    let mut spin_nodes = vec![];
    let mut counts = BTreeMap::<String, usize>::new();
    for e in &ents {
        let class = e.get("classname").map(|s| s.to_lowercase()).unwrap_or_default();
        if class == "worldspawn" {
            continue;
        }
        *counts.entry(class.clone()).or_default() += 1;
        let extras: Map<String, Value> = e.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let label = match e.get("targetname") {
            Some(t) => format!("{class}:{t}"),
            None => class.clone(),
        };
        if let Some(mi) = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|m| m.parse::<usize>().ok()) {
            // Brush models are stored in world space.
            if let Some(n) = model_node(&mut g, mi, label)? {
                g.nodes[n]["extras"] = Value::Object(extras);
                g.add_child(ent_root, n);
            }
            continue;
        }
        let Some(origin) = e.get("origin").and_then(|o| vec3_of(o)) else { continue };
        let yaw = e.get("angle").and_then(|a| a.parse::<f32>().ok()).unwrap_or(0.0);
        let rot = Quat::from_rotation_y(yaw.to_radians());
        let n = g.node(json!({"name": label, "translation": q2g(origin).to_array(),
                              "rotation": rot.to_array(), "extras": extras}));
        g.add_child(ent_root, n);
        let models = item_models(&class);
        if !models.is_empty() && !e.contains_key("notfree") {
            let spin = g.node(json!({"name": "spin"}));
            g.add_child(n, spin);
            spin_nodes.push(spin);
            for m in models {
                let mesh = *item_mesh_cache.entry(m).or_insert_with(|| md3_static(&mut g, ctx, &out, m));
                if let Some(mesh) = mesh {
                    let mn = g.node(json!({"name": m.rsplit('/').next().unwrap(), "mesh": mesh}));
                    g.add_child(spin, mn);
                }
            }
        }
    }
    if !spin_nodes.is_empty() {
        // Items spin once every 2.048 s in cgame.
        let times: Vec<f32> = (0..=4).map(|i| i as f32 * 0.512).collect();
        let rot: Vec<f32> =
            (0..=4).flat_map(|i| Quat::from_rotation_y(i as f32 * std::f32::consts::FRAC_PI_2).to_array()).collect();
        let mut samplers = vec![];
        let mut channels = vec![];
        let input = g.acc_f32(&times, "SCALAR", true, None);
        let output = g.acc_f32(&rot, "VEC4", false, None);
        samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
        for n in spin_nodes {
            channels.push(json!({"sampler": 0, "target": {"node": n, "path": "rotation"}}));
        }
        g.animations.push(json!({"name": "item_spin", "channels": channels, "samplers": samplers}));
    }

    let world = ents.iter().find(|e| e.get("classname").map(|c| c == "worldspawn").unwrap_or(false));
    let class_is = |e: &&BTreeMap<String, String>, c: &str| e.get("classname").map(|x| x == c).unwrap_or(false);
    let spawn = ents
        .iter()
        .find(|e| class_is(e, "info_player_deathmatch"))
        .or_else(|| ents.iter().find(|e| class_is(e, "info_player_start")))
        .and_then(|e| {
            Some((vec3_of(e.get("origin")?)?, e.get("angle").and_then(|a| a.parse::<f32>().ok()).unwrap_or(0.0)))
        });
    g.extras = json!({
        "map": name,
        "message": world.and_then(|w| w.get("message")),
        "music": world.and_then(|w| w.get("music")),
        "fogs": bsp.fogs,
    });
    ctx.write(&out, &g.to_glb())?;
    let ents_path = format!("{mod_name}/maps/{name}.entities.json");
    ctx.write(&ents_path, serde_json::to_string_pretty(&ents)?.as_bytes())?;

    let levelshot = ctx.vfs.resolve_image(&format!("levelshots/{name}")).map(|p| ctx.vfs.out_path(&p, "png"));
    Ok(json!({
        "name": name, "glb": out, "entities": ents_path, "lightmap": (n_lm > 0).then_some(lm_path),
        "levelshot": levelshot,
        "title": world.and_then(|w| w.get("message")),
        "music": world.and_then(|w| w.get("music")).map(|m| m.to_lowercase().replace('\\', "/")),
        "spawn": spawn.map(|(o, a)| json!({"origin": q2g(o + Vec3::new(0., 0., 40.)).to_array(), "yaw": a})),
        "shaders": shaders_used.keys().collect::<Vec<_>>(),
        "entityCounts": counts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Past its own vertices' budget a map shares vertex ranges; within it, past `MAX_EMITTED` it is too large.
    #[test]
    fn geometry_budget_messages() {
        assert_eq!(over_budget(100, 100), None);
        assert!(over_budget(101, 100).is_some_and(|m| m.contains("share vertex ranges")));
        assert!(over_budget(MAX_EMITTED + 1, MAX_EMITTED * 2).is_some_and(|m| m.contains("geometry limit")));
        assert!(over_budget(MAX_EMITTED + 1, MAX_EMITTED).is_some_and(|m| m.contains("share vertex ranges")));
        assert_eq!(over_budget(MAX_EMITTED, MAX_EMITTED * 2), None);
    }
}
