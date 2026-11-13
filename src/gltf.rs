//! Minimal glTF 2.0 binary (.glb) writer. Images are referenced by relative URI so
//! textures are shared between files instead of duplicated.

use glam::{Mat3, Quat, Vec3};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Quake units are roughly inches; glTF wants meters.
pub const SCALE: f32 = 0.0254;

/// Quake (Z up, X forward) -> glTF (Y up). A rotation, so handedness and winding are kept.
pub fn q2g(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y) * SCALE
}
pub fn q2g_dir(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y)
}

/// Convert a Quake tag/rotation basis (rows = axes) into a glTF quaternion.
pub fn q2g_rot(axis: [Vec3; 3]) -> Quat {
    // Columns are the child's axes expressed in the parent frame (Gram-Schmidt for safety).
    let x = axis[0].normalize_or(Vec3::X);
    let y = (axis[1] - x * axis[1].dot(x)).normalize_or(Vec3::Y);
    let z = x.cross(y);
    let r = Mat3::from_cols(x, y, z);
    let c = Mat3::from_cols(Vec3::new(1., 0., 0.), Vec3::new(0., 0., -1.), Vec3::new(0., 1., 0.));
    Quat::from_mat3(&(c * r * c.transpose())).normalize()
}

pub const ARRAY_BUFFER: u32 = 34962;
pub const ELEMENT_ARRAY_BUFFER: u32 = 34963;

#[derive(Default)]
pub struct Gltf {
    pub nodes: Vec<Value>,
    pub meshes: Vec<Value>,
    pub materials: Vec<Value>,
    pub textures: Vec<Value>,
    pub images: Vec<Value>,
    pub accessors: Vec<Value>,
    pub buffer_views: Vec<Value>,
    pub animations: Vec<Value>,
    pub roots: Vec<usize>,
    pub extras: Value,
    bin: Vec<u8>,
    tex_cache: HashMap<String, usize>,
    pub mat_cache: HashMap<String, usize>,
}

impl Gltf {
    pub fn new() -> Self {
        Gltf { extras: json!({}), ..Default::default() }
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.buffer_views.push(v);
        self.buffer_views.len() - 1
    }

    /// Float accessor. `ty` is SCALAR/VEC2/VEC3/VEC4.
    pub fn acc_f32(&mut self, data: &[f32], ty: &str, with_bounds: bool, target: Option<u32>) -> usize {
        let n = match ty {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => panic!("bad accessor type"),
        };
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let bv = self.view(&bytes, target);
        let mut a = json!({"bufferView": bv, "componentType": 5126, "count": data.len() / n, "type": ty});
        if with_bounds {
            let mut mn = vec![f32::MAX; n];
            let mut mx = vec![f32::MIN; n];
            for c in data.chunks(n) {
                for i in 0..n {
                    mn[i] = mn[i].min(c[i]);
                    mx[i] = mx[i].max(c[i]);
                }
            }
            if data.is_empty() {
                mn = vec![0.; n];
                mx = vec![0.; n];
            }
            a["min"] = json!(mn);
            a["max"] = json!(mx);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    pub fn acc_vec3(&mut self, data: &[Vec3], with_bounds: bool) -> usize {
        let flat: Vec<f32> = data.iter().flat_map(|v| v.to_array()).collect();
        self.acc_f32(&flat, "VEC3", with_bounds, Some(ARRAY_BUFFER))
    }

    pub fn acc_indices(&mut self, idx: &[u32]) -> usize {
        let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        let bv = self.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
        self.accessors.push(json!({"bufferView": bv, "componentType": 5125, "count": idx.len(), "type": "SCALAR"}));
        self.accessors.len() - 1
    }

    /// Texture referencing an external image by URI (relative to the .glb).
    pub fn texture(&mut self, uri: &str) -> usize {
        if let Some(&t) = self.tex_cache.get(uri) {
            return t;
        }
        let mime = if uri.ends_with(".jpg") { "image/jpeg" } else { "image/png" };
        self.images.push(json!({"uri": uri, "mimeType": mime}));
        self.textures.push(json!({"source": self.images.len() - 1, "sampler": 0}));
        let t = self.textures.len() - 1;
        self.tex_cache.insert(uri.to_string(), t);
        t
    }

    pub fn material(&mut self, key: &str, make: impl FnOnce(&mut Self) -> Value) -> usize {
        if let Some(&m) = self.mat_cache.get(key) {
            return m;
        }
        let v = make(self);
        self.materials.push(v);
        let m = self.materials.len() - 1;
        self.mat_cache.insert(key.to_string(), m);
        m
    }

    pub fn mesh(&mut self, v: Value) -> usize {
        self.meshes.push(v);
        self.meshes.len() - 1
    }

    pub fn node(&mut self, v: Value) -> usize {
        self.nodes.push(v);
        self.nodes.len() - 1
    }

    pub fn add_child(&mut self, parent: usize, child: usize) {
        let n = &mut self.nodes[parent];
        if n.get("children").is_none() {
            n["children"] = json!([]);
        }
        n["children"].as_array_mut().unwrap().push(json!(child));
    }

    pub fn to_glb(&self) -> Vec<u8> {
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "okconv-q3"},
            "scene": 0,
            "scenes": [{"nodes": self.roots}],
            "nodes": self.nodes,
            "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
            "buffers": [{"byteLength": self.bin.len()}],
        });
        let put = |d: &mut Value, k: &str, v: &Vec<Value>| {
            if !v.is_empty() {
                d[k] = json!(v);
            }
        };
        put(&mut doc, "meshes", &self.meshes);
        put(&mut doc, "materials", &self.materials);
        put(&mut doc, "textures", &self.textures);
        put(&mut doc, "images", &self.images);
        put(&mut doc, "accessors", &self.accessors);
        put(&mut doc, "bufferViews", &self.buffer_views);
        put(&mut doc, "animations", &self.animations);
        if self.extras.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            doc["scenes"][0]["extras"] = self.extras.clone();
        }
        let mut js = serde_json::to_vec(&doc).unwrap();
        while !js.len().is_multiple_of(4) {
            js.push(b' ');
        }
        let mut bin = self.bin.clone();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + js.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(js.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&js);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }
}
