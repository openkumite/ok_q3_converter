//! MD3 (IDP3 v15) model parser.

use anyhow::{Result, bail};
use glam::Vec3;

pub struct Tag {
    pub name: String,
    pub origin: Vec3,
    pub axis: [Vec3; 3],
}

pub struct Surface {
    pub name: String,
    pub shaders: Vec<String>,
    pub tris: Vec<[u32; 3]>,
    pub st: Vec<[f32; 2]>,
    /// frames[f][v] = (position, normal) in Quake space
    pub frames: Vec<Vec<(Vec3, Vec3)>>,
}

pub struct Md3 {
    pub num_frames: usize,
    /// tags[frame][tag]
    pub tags: Vec<Vec<Tag>>,
    pub surfaces: Vec<Surface>,
}

impl Md3 {
    pub fn tag(&self, frame: usize, name: &str) -> Option<&Tag> {
        self.tags.get(frame.min(self.num_frames.saturating_sub(1)))?.iter().find(|t| t.name == name)
    }
    pub fn tag_names(&self) -> Vec<String> {
        self.tags.first().map(|t| t.iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
    }
}

struct R<'a>(&'a [u8]);
impl R<'_> {
    fn i32(&self, o: usize) -> Result<i32> {
        let b = self.0.get(o..o.saturating_add(4)).ok_or_else(|| anyhow::anyhow!("md3 truncated"))?;
        Ok(i32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u(&self, o: usize) -> Result<usize> {
        let v = self.i32(o)?;
        if v < 0 {
            bail!("negative count/offset in md3");
        }
        Ok(v as usize)
    }
    fn f32(&self, o: usize) -> Result<f32> {
        Ok(f32::from_bits(self.i32(o)? as u32))
    }
    fn i16(&self, o: usize) -> Result<i16> {
        let b = self.0.get(o..o.saturating_add(2)).ok_or_else(|| anyhow::anyhow!("md3 truncated"))?;
        Ok(i16::from_le_bytes(b.try_into().unwrap()))
    }
    fn vec3(&self, o: usize) -> Result<Vec3> {
        Ok(Vec3::new(self.f32(o)?, self.f32(o + 4)?, self.f32(o + 8)?))
    }
    fn str(&self, o: usize, n: usize) -> Result<String> {
        let b = self.0.get(o..o.saturating_add(n)).ok_or_else(|| anyhow::anyhow!("md3 truncated"))?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&b[..end]).to_lowercase().replace('\\', "/"))
    }
}

/// Offset of record `i` of `size` bytes at `base`; the counts and offsets come from the file, so overflow
/// (usize is 32 bits on wasm32) or a record past the end is an error.
fn rec(data: &[u8], base: usize, i: usize, size: usize) -> Result<usize> {
    match i.checked_mul(size).and_then(|o| o.checked_add(base)) {
        Some(o) if o <= data.len() => Ok(o),
        _ => bail!("md3 truncated"),
    }
}

fn decode_normal(n: u16) -> Vec3 {
    let lat = ((n >> 8) & 0xff) as f32 * std::f32::consts::TAU / 256.0;
    let lng = (n & 0xff) as f32 * std::f32::consts::TAU / 256.0;
    Vec3::new(lat.cos() * lng.sin(), lat.sin() * lng.sin(), lng.cos())
}

pub fn parse(data: &[u8]) -> Result<Md3> {
    let r = R(data);
    if data.len() < 108 || &data[0..4] != b"IDP3" || r.i32(4)? != 15 {
        bail!("not an IDP3 v15 model");
    }
    let num_frames = r.u(76)?;
    let num_tags = r.u(80)?;
    let num_surfaces = r.u(84)?;
    let ofs_tags = r.u(96)?;
    let ofs_surfaces = r.u(100)?;
    // The loops below run on file-supplied counts and may read nothing per pass (no tags, no vertices):
    // each frame has a 56-byte header, and a surface has no more frames than the model (as Q3 expects).
    if num_frames > data.len() / 56 {
        bail!("md3 truncated");
    }

    let mut tags = vec![];
    for f in 0..num_frames {
        let mut ft = vec![];
        for t in 0..num_tags {
            let o =
                rec(data, ofs_tags, f.checked_mul(num_tags).and_then(|i| i.checked_add(t)).unwrap_or(usize::MAX), 112)?;
            ft.push(Tag {
                name: r.str(o, 64)?,
                origin: r.vec3(o + 64)?,
                axis: [r.vec3(o + 76)?, r.vec3(o + 88)?, r.vec3(o + 100)?],
            });
        }
        tags.push(ft);
    }

    let mut surfaces = vec![];
    let mut so = ofs_surfaces;
    // Records are checked against data.len() one by one, but nothing stops every surface from pointing at the same
    // bytes (and a surface with no vertices still costs a Vec per frame), so the total could grow with the square
    // of the file size. A real file stores each shader, triangle, st and xyz record once, at 8 bytes or more, so
    // all surfaces together read at most data.len() / 8 records (counting a vertexless frame as one).
    let mut total_records = 0usize;
    for _ in 0..num_surfaces {
        if so > data.len() || &data.get(so..so + 4).unwrap_or(b"") != b"IDP3" {
            bail!("bad surface ident");
        }
        let name = r.str(so + 4, 64)?;
        let s_frames = r.u(so + 72)?;
        let num_shaders = r.u(so + 76)?;
        let num_verts = r.u(so + 80)?;
        let num_tris = r.u(so + 84)?;
        let ofs_tris = so.saturating_add(r.u(so + 88)?);
        let ofs_shaders = so.saturating_add(r.u(so + 92)?);
        let ofs_st = so.saturating_add(r.u(so + 96)?);
        let ofs_xyz = so.saturating_add(r.u(so + 100)?);
        let ofs_end = r.u(so + 104)?;
        if s_frames > num_frames {
            bail!("surface has more frames than the model");
        }
        total_records = total_records
            .saturating_add(num_shaders)
            .saturating_add(num_tris)
            .saturating_add(num_verts)
            .saturating_add(s_frames.saturating_mul(num_verts.max(1)));
        if total_records > data.len() / 8 {
            bail!("md3 truncated");
        }

        let shaders = (0..num_shaders).map(|i| r.str(rec(data, ofs_shaders, i, 68)?, 64)).collect::<Result<_>>()?;
        // The counts come from the file: reserve no more records than the data can hold.
        let mut tris = Vec::with_capacity(num_tris.min(data.len() / 12));
        for t in 0..num_tris {
            let o = rec(data, ofs_tris, t, 12)?;
            let tri = [r.u(o)? as u32, r.u(o + 4)? as u32, r.u(o + 8)? as u32];
            if tri.iter().any(|&i| i as usize >= num_verts) {
                bail!("triangle index out of range");
            }
            tris.push(tri);
        }
        let mut st = Vec::with_capacity(num_verts.min(data.len() / 8));
        for v in 0..num_verts {
            let o = rec(data, ofs_st, v, 8)?;
            st.push([r.f32(o)?, r.f32(o + 4)?]);
        }
        let mut frames = Vec::with_capacity(s_frames.min(data.len() / 8));
        for f in 0..s_frames {
            let mut verts = Vec::with_capacity(num_verts.min(data.len() / 8));
            for v in 0..num_verts {
                let o = rec(
                    data,
                    ofs_xyz,
                    f.checked_mul(num_verts).and_then(|i| i.checked_add(v)).unwrap_or(usize::MAX),
                    8,
                )?;
                let p = Vec3::new(r.i16(o)? as f32, r.i16(o + 2)? as f32, r.i16(o + 4)? as f32) / 64.0;
                let n = decode_normal(r.i16(o + 6)? as u16);
                verts.push((p, n));
            }
            frames.push(verts);
        }
        surfaces.push(Surface { name, shaders, tris, st, frames });
        if ofs_end == 0 {
            bail!("zero-length surface");
        }
        so = so.saturating_add(ofs_end);
    }
    Ok(Md3 { num_frames, tags, surfaces })
}

#[cfg(test)]
mod tests {
    /// 2^31 - 1 frames with no tags is an error, not 2^31 empty frames.
    #[test]
    fn huge_frames_no_tags() {
        let mut d = vec![0u8; 108];
        d[0..4].copy_from_slice(b"IDP3");
        d[4..8].copy_from_slice(&15i32.to_le_bytes());
        d[76..80].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(super::parse(&d).is_err());
    }

    /// A surface claiming 2^31 - 1 frames with no vertices is an error, not 2^31 empty frames.
    #[test]
    fn huge_surface_frames_no_verts() {
        let mut d = vec![0u8; 108 + 108];
        d[0..4].copy_from_slice(b"IDP3");
        d[4..8].copy_from_slice(&15i32.to_le_bytes());
        d[84..88].copy_from_slice(&1i32.to_le_bytes()); // one surface
        d[100..104].copy_from_slice(&108i32.to_le_bytes());
        d[108..112].copy_from_slice(b"IDP3");
        d[108 + 72..108 + 76].copy_from_slice(&i32::MAX.to_le_bytes());
        d[108 + 104..108 + 108].copy_from_slice(&108i32.to_le_bytes());
        assert!(super::parse(&d).is_err());
    }

    /// Many 108-byte surfaces with no vertices, each with every frame of the model, are an error, not an
    /// allocation quadratic in the file size.
    #[test]
    fn many_empty_surfaces() {
        let num_surfaces = 2000;
        let num_frames = 3000i32; // within data.len() / 56
        let mut d = vec![0u8; 108 + 108 * num_surfaces];
        d[0..4].copy_from_slice(b"IDP3");
        d[4..8].copy_from_slice(&15i32.to_le_bytes());
        d[76..80].copy_from_slice(&num_frames.to_le_bytes());
        d[84..88].copy_from_slice(&(num_surfaces as i32).to_le_bytes());
        d[100..104].copy_from_slice(&108i32.to_le_bytes());
        for s in 0..num_surfaces {
            let so = 108 + 108 * s;
            d[so..so + 4].copy_from_slice(b"IDP3");
            d[so + 72..so + 76].copy_from_slice(&num_frames.to_le_bytes());
            d[so + 104..so + 108].copy_from_slice(&108i32.to_le_bytes());
        }
        assert!(super::parse(&d).is_err());
    }

    /// Surfaces whose st and xyz records all point at one shared region (each in bounds on its own) are an
    /// error, not every surface decoding the whole region again.
    #[test]
    fn surfaces_share_one_region() {
        let len = 64 << 10;
        let region = len / 2;
        let num_surfaces = (region - 108) / 108;
        let num_verts = (len - region) / 8;
        let mut d = vec![0u8; len];
        let w = |d: &mut Vec<u8>, o: usize, v: usize| d[o..o + 4].copy_from_slice(&(v as i32).to_le_bytes());
        d[0..4].copy_from_slice(b"IDP3");
        w(&mut d, 4, 15);
        w(&mut d, 76, 1);
        w(&mut d, 84, num_surfaces);
        w(&mut d, 100, 108);
        for s in 0..num_surfaces {
            let so = 108 + 108 * s;
            d[so..so + 4].copy_from_slice(b"IDP3");
            w(&mut d, so + 72, 1);
            w(&mut d, so + 80, num_verts);
            w(&mut d, so + 96, region - so);
            w(&mut d, so + 100, region - so);
            w(&mut d, so + 104, 108);
        }
        assert!(super::parse(&d).is_err());
    }

    /// A surface claiming 2^31 - 1 triangles, vertices and frames is an error, not a huge allocation.
    #[test]
    fn huge_counts() {
        let mut d = vec![0u8; 108 + 108];
        d[0..4].copy_from_slice(b"IDP3");
        d[4..8].copy_from_slice(&15i32.to_le_bytes());
        d[84..88].copy_from_slice(&1i32.to_le_bytes()); // one surface
        d[100..104].copy_from_slice(&108i32.to_le_bytes());
        d[108..112].copy_from_slice(b"IDP3");
        for o in [72, 80, 84] {
            d[108 + o..108 + o + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        }
        for o in [88, 92, 96, 100] {
            d[108 + o..108 + o + 4].copy_from_slice(&108i32.to_le_bytes());
        }
        assert!(super::parse(&d).is_err());
    }
}
