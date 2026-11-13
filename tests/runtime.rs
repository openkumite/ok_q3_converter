//! `runtime::Converter` over synthetic paks (a tiny IBSP v46 map, a shader script, TGA / JPG images, a second pak
//! overriding a file), and an ignored test over the real Quake III Arena paks against the CLI's output.

use ok_q3_converter::runtime::{Converter, Error, MemorySink, Paks, crc32};
use ok_q3_converter::vfs::ReadSeek;
use serde_json::Value;
use std::io::{Cursor, Write};

/// A pk3 holding `files` (path, bytes).
fn pk3(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn image(rgb: [u8; 3], fmt: image::ImageFormat) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb(rgb)));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, fmt).unwrap();
    buf.into_inner()
}

/// One world model of one triangle surface with shader `shader`, a lightmap, worldspawn and a spawn point.
fn tiny_bsp(shader: &str) -> Vec<u8> {
    let mut lumps: Vec<Vec<u8>> = vec![Vec::new(); 17];
    lumps[0] = b"{\n\"classname\" \"worldspawn\"\n\"message\" \"Tiny\"\n}\n{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"0 0 24\"\n}\n\0".to_vec();
    let mut sh = vec![0u8; 72];
    sh[..shader.len()].copy_from_slice(shader.as_bytes());
    lumps[1] = sh;
    let i32s = |v: &[i32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let f32s = |v: &[f32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let mut model = f32s(&[0.0; 6]);
    model.extend(i32s(&[0, 1, 0, 0]));
    lumps[7] = model;
    for (x, y) in [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)] {
        lumps[10].extend(f32s(&[x, y, 0.0, x / 64.0, y / 64.0, 0.5, 0.5, 0.0, 0.0, 1.0]));
        lumps[10].extend([255, 255, 255, 255]);
    }
    lumps[11] = i32s(&[0, 1, 2]);
    let mut surf = i32s(&[0, -1, 1, 0, 3, 0, 3, 0]);
    surf.resize(104, 0);
    lumps[13] = surf;
    lumps[14] = vec![64; 128 * 128 * 3];
    ibsp(&lumps)
}

/// An IBSP v46 file of the 17 `lumps`.
fn ibsp(lumps: &[Vec<u8>]) -> Vec<u8> {
    let i32s = |v: &[i32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let mut d = b"IBSP".to_vec();
    d.extend(46i32.to_le_bytes());
    let mut off = 8 + 17 * 8;
    for l in lumps {
        d.extend(i32s(&[off as i32, l.len() as i32]));
        off += l.len();
    }
    for l in lumps {
        d.extend(l);
    }
    d
}

const SCRIPT: &str = "textures/test/wall\n{\n\t{\n\t\tmap $lightmap\n\t}\n\t{\n\t\tmap textures/test/wall.tga\n\t\tblendfunc filter\n\t}\n\t{\n\t\tanimMap 2 textures/test/anim1.tga textures/test/anim2.jpg\n\t}\n}\n";

fn paks(list: Vec<(&str, Vec<u8>)>) -> Vec<(String, Box<dyn ReadSeek>)> {
    list.into_iter().map(|(n, d)| (n.to_string(), Box::new(Cursor::new(d)) as Box<dyn ReadSeek>)).collect()
}

fn test_paks() -> Vec<(String, Box<dyn ReadSeek>)> {
    use image::ImageFormat::{Jpeg, Tga};
    let base = pk3(&[
        ("maps/tiny.bsp", tiny_bsp("textures/test/wall")),
        ("scripts/test.shader", SCRIPT.as_bytes().to_vec()),
        ("textures/test/wall.tga", image([255, 0, 0], Tga)),
        ("textures/test/anim1.tga", image([0, 255, 0], Tga)),
        ("textures/test/anim2.jpg", image([0, 0, 255], Jpeg)),
        ("textures/test/unused.tga", image([1, 2, 3], Tga)),
        ("env/sky1_ft.jpg", image([9, 9, 9], Jpeg)),
        ("levelshots/tiny.jpg", image([7, 7, 7], Jpeg)),
    ]);
    // later in load order (names compare case-insensitively) and with an upper case path: wins
    let over = pk3(&[("Textures/Test/WALL.TGA", image([255, 255, 0], Tga))]);
    paks(vec![("pak1.PK3", over), ("PAK0.pk3", base)])
}

#[test]
fn converts_a_tiny_pak_stack() {
    let p = Paks::open(test_paks()).unwrap();
    assert_eq!(p.maps(), ["tiny"]);
    assert_eq!(p.names(), ["baseq3/PAK0.pk3", "baseq3/pak1.PK3"]);
    let c = Converter::new(p);
    let total = c.total();
    let mut sink = MemorySink::default();
    let mut seen = 0;
    let sum = c
        .run(&mut sink, &mut |pr| {
            assert_eq!((pr.done, pr.total), (seen, total));
            seen += 1;
            true
        })
        .unwrap();
    assert_eq!(seen, total);
    assert_eq!(sum.maps, ["tiny"]);
    let names: Vec<&str> = sink.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "baseq3/env/sky1_ft.png",
            "baseq3/levelshots/tiny.png",
            "baseq3/maps/tiny.entities.json",
            "baseq3/maps/tiny.glb",
            "baseq3/maps/tiny_lightmap.png",
            "baseq3/textures/test/anim1.png",
            "baseq3/textures/test/anim2.png",
            "baseq3/textures/test/wall.png",
            "manifest.json",
            "shaders.json",
        ]
    );
    // the later pak's wall
    let wall = image::load_from_memory(&sink.files["baseq3/textures/test/wall.png"]).unwrap().to_rgb8();
    assert_eq!(wall.get_pixel(0, 0).0, [255, 255, 0]);
    // the map names its texture relative to itself
    let glb = String::from_utf8_lossy(&sink.files["baseq3/maps/tiny.glb"]).into_owned();
    assert!(glb.contains("\"uri\":\"../../baseq3/textures/test/wall.png\""), "{glb}");
    assert!(glb.contains("\"uri\":\"../../baseq3/maps/tiny_lightmap.png\""));
    // the manifest lists every other file with its size and CRC-32
    let m: Value = serde_json::from_slice(&sink.files["manifest.json"]).unwrap();
    assert_eq!(
        (m["game"].as_str(), m["format"].as_u64(), m["source"].as_str()),
        (Some("Quake III Arena"), Some(1), Some("pk3"))
    );
    assert_eq!(m["maps"][0]["name"], "tiny");
    assert_eq!(m["skipped"].as_array().map(Vec::len), Some(0));
    let files = m["files"].as_array().unwrap();
    assert_eq!(files.len(), sink.files.len() - 1);
    for f in files {
        let data = &sink.files[f["path"].as_str().unwrap()];
        assert_eq!(f["size"].as_u64(), Some(data.len() as u64));
        assert_eq!(f["crc32"].as_u64(), Some(crc32(data) as u64));
    }
    assert_eq!(sum.files.len(), files.len());
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

/// Patch surfaces that all tessellate the same vertex range (a few hundred bytes once deflated) make the map an
/// error, listed in `skipped`, not output growing with the square of the input.
#[test]
fn surfaces_share_one_vertex_range() {
    let (num_surfaces, h) = (1000, 31);
    let mut lumps: Vec<Vec<u8>> = vec![Vec::new(); 17];
    lumps[0] = b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec();
    let mut sh = vec![0u8; 72];
    sh[..18].copy_from_slice(b"textures/test/wall");
    lumps[1] = sh;
    let i32s = |v: &[i32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let mut model = vec![0u8; 24];
    model.extend(i32s(&[0, num_surfaces, 0, 0]));
    lumps[7] = model;
    lumps[10] = vec![0; 44 * 3 * h as usize];
    for _ in 0..num_surfaces {
        // shader 0, no fog, patch, verts 0..3h, no indexes, no lightmap, 3 x h control points
        let mut surf = i32s(&[0, -1, 2, 0, 3 * h, 0, 0, -1]);
        surf.resize(96, 0);
        surf.extend(i32s(&[3, h]));
        lumps[13].extend(surf);
    }
    let pak = pk3(&[("maps/shared.bsp", ibsp(&lumps))]);
    let mut sink = MemorySink::default();
    Converter::new(Paks::open(paks(vec![("pak0.pk3", pak)])).unwrap()).run(&mut sink, &mut |_| true).unwrap();
    let m: Value = serde_json::from_slice(&sink.files["manifest.json"]).unwrap();
    assert_eq!(m["skipped"][0]["path"], "maps/shared.bsp", "{}", m["skipped"]);
    assert!(m["skipped"][0]["error"].as_str().is_some_and(|e| e.contains("share vertex ranges")), "{}", m["skipped"]);
    assert!(!sink.files.contains_key("baseq3/maps/shared.glb"));
}

/// A triangle surface whose index lies outside its vertices (here -1, 0xFFFFFFFF as u32, which before the check went
/// into the GLB as an index; with `base` > 0 it would also overflow `base + i`) is left out; the map converts without
/// it.
#[test]
fn surface_index_out_of_range() {
    let mut bsp = tiny_bsp("textures/test/wall");
    // the index lump is the 12th (number 11): its offset in the header
    let off = i32::from_le_bytes(bsp[8 + 11 * 8..12 + 11 * 8].try_into().unwrap()) as usize;
    bsp[off + 8..off + 12].copy_from_slice(&(-1i32).to_le_bytes());
    let pak = pk3(&[("maps/tiny.bsp", bsp), ("scripts/test.shader", SCRIPT.as_bytes().to_vec())]);
    let mut sink = MemorySink::default();
    Converter::new(Paks::open(paks(vec![("pak0.pk3", pak)])).unwrap()).run(&mut sink, &mut |_| true).unwrap();
    let m: Value = serde_json::from_slice(&sink.files["manifest.json"]).unwrap();
    assert_eq!(m["skipped"].as_array().map(Vec::len), Some(0), "{}", m["skipped"]);
    let glb = String::from_utf8_lossy(&sink.files["baseq3/maps/tiny.glb"]).into_owned();
    assert!(!glb.contains("\"primitives\""), "the surface is left out");
}

#[test]
fn cancel_no_maps_and_not_a_zip() {
    let c = Converter::new(Paks::open(test_paks()).unwrap());
    let mut calls = 0;
    let r = c.run(&mut MemorySink::default(), &mut |_| {
        calls += 1;
        calls < 3
    });
    assert!(matches!(r, Err(Error::Cancelled)));
    let only_images = pk3(&[("textures/test/wall.tga", image([1, 1, 1], image::ImageFormat::Tga))]);
    assert!(matches!(Paks::open(paks(vec![("pak0.pk3", only_images)])), Err(Error::NoMaps)));
    assert!(matches!(Paks::open(paks(vec![("pak0.pk3", b"not a zip".to_vec())])), Err(Error::Format(_))));
}

/// The real paks (`Q3_BASEQ3`, the `baseq3` folder) against the CLI's full output of them (`Q3_CLI_OUTPUT`,
/// default this repo's `output/`): every file `runtime::Converter` writes is the CLI's file at the same path.
/// `cargo test --release --features runtime -- --ignored --nocapture`
#[test]
#[ignore]
fn real_paks_match_the_cli() {
    let dir = std::env::var("Q3_BASEQ3").expect("set Q3_BASEQ3 to a Quake III Arena baseq3 folder");
    let cli = std::path::PathBuf::from(
        std::env::var("Q3_CLI_OUTPUT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/output").into()),
    );
    let mut list: Vec<(String, Box<dyn ReadSeek>)> = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("Q3_BASEQ3 {dir}: {e}")) {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pk3")) {
            let f = std::io::BufReader::new(std::fs::File::open(&p).unwrap());
            list.push((p.file_name().unwrap().to_string_lossy().into_owned(), Box::new(f)));
        }
    }
    let t = std::time::Instant::now();
    let p = Paks::open(list).unwrap();
    let opened = t.elapsed();
    let mut sink = MemorySink::default();
    let sum = Converter::new(p).run(&mut sink, &mut |_| true).unwrap();
    let bytes: u64 = sum.files.values().map(|f| f.0).sum();
    println!(
        "opened in {:.2?}, converted in {:.2?}: {} maps, {} files, {} bytes (+ manifest.json {} bytes)",
        opened,
        t.elapsed(),
        sum.maps.len(),
        sum.files.len(),
        bytes,
        sink.files["manifest.json"].len()
    );
    let m: Value = serde_json::from_slice(&sink.files["manifest.json"]).unwrap();
    assert_eq!(m["skipped"].as_array().map(Vec::len), Some(0), "{}", m["skipped"]);
    for (path, data) in &sink.files {
        if path == "manifest.json" {
            continue;
        }
        let theirs = std::fs::read(cli.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(theirs == *data, "{path} differs from the CLI's");
    }
}
