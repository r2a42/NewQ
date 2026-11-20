//! Venue models made in the tests themselves: small glTF, GLB and OBJ files
//! written to a temporary folder.

use super::*;
use crate::stage3d::{LengthUnit, Stage3d, UpAxis};
use serde_json::json;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!(
            "newq-stage3d-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.0.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn venue() -> Venue {
    Venue::new(PathBuf::new())
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| close(a[i], b[i]))
}

/// A `w` x `h` PNG of one colour, with a red top-left pixel.
fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn pad4(v: &mut Vec<u8>, with: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(with);
    }
}

/// A GLB from its JSON and its binary chunk.
fn glb(json: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut j = serde_json::to_vec(json).unwrap();
    pad4(&mut j, b' ');
    let mut b = bin.to_vec();
    pad4(&mut b, 0);
    let total = 12 + 8 + j.len() + if b.is_empty() { 0 } else { 8 + b.len() };
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&j);
    if !b.is_empty() {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&b);
    }
    out
}

/// One triangle (0,0,0), (1,0,0), (0,1,0) facing +Z, with UVs and a
/// picture: the binary data and the JSON's buffer views and accessors.
struct Tri {
    bin: Vec<u8>,
    png_len: usize,
}

fn tri(picture: &[u8]) -> Tri {
    let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // 36
    bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0])); // 24, at 36
    bin.extend([0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes())); // 6, at 60
    bin.extend([0, 0]); // to 68
    bin.extend_from_slice(picture); // at 68
    Tri {
        bin,
        png_len: picture.len(),
    }
}

fn tri_json(t: &Tri, buffer: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut j = json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [
            { "translation": [10.0, 0.0, 0.0], "children": [1] },
            { "mesh": 0, "scale": [2.0, 2.0, 2.0] }
        ],
        "meshes": [{ "primitives": [{
            "attributes": { "POSITION": 0, "TEXCOORD_0": 1 },
            "indices": 2,
            "material": 0
        }]}],
        "materials": [{ "pbrMetallicRoughness": {
            "baseColorFactor": [1.0, 0.5, 0.25, 1.0],
            "baseColorTexture": { "index": 0 }
        }}],
        "textures": [{ "source": 0 }],
        "images": [{ "bufferView": 3, "mimeType": "image/png" }],
        "buffers": [buffer],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
            { "buffer": 0, "byteOffset": 36, "byteLength": 24 },
            { "buffer": 0, "byteOffset": 60, "byteLength": 6 }
        ],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
              "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] },
            { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" },
            { "bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR" }
        ]
    });
    if t.png_len > 0 {
        j["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "buffer": 0, "byteOffset": 68, "byteLength": t.png_len }));
    }
    if let (Some(o), Some(e)) = (j.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    j
}

#[test]
fn a_glb_gives_its_meshes_placed_by_their_nodes_with_colour_and_picture() {
    let d = Dir::new("glb");
    let t = tri(&png(4, 2, [200, 200, 200, 255]));
    let json = tri_json(&t, json!({ "byteLength": t.bin.len() }), json!({}));
    let path = d.write("venue.glb", glb(&json, &t.bin));
    let v = load_venue(&path, &venue()).unwrap();
    assert_eq!(v.triangles, 1);
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    assert_eq!(v.parts.len(), 1);
    let m = &v.parts[0].mesh;
    // Scaled by 2 by its node, moved 10 along X by its parent.
    assert!(close3(m.positions[0], [10.0, 0.0, 0.0]));
    assert!(close3(m.positions[1], [12.0, 0.0, 0.0]));
    assert!(close3(m.positions[2], [10.0, 2.0, 0.0]));
    // No normals in the file: worked out, facing +Z.
    assert!(m.normals.iter().all(|n| close3(*n, [0.0, 0.0, 1.0])));
    assert_eq!(m.uvs[1], [1.0, 0.0]);
    let b = v.bounds.unwrap();
    assert!(close3(b.min, [10.0, 0.0, 0.0]) && close3(b.max, [12.0, 2.0, 0.0]));
    // The picture, with the base colour multiplied in.
    let Paint::Image {
        rgba,
        width,
        height,
        key,
    } = &v.parts[0].paint
    else {
        panic!("{:?}", v.parts[0].paint);
    };
    assert_eq!((*width, *height), (4, 2));
    assert_eq!(&rgba[0..4], &[255, 0, 0, 255]); // red top-left, red kept
    let p = &rgba[4..8];
    assert_eq!(p[0], 200);
    assert!(p[1] < 200 && p[2] < p[1], "{p:?}");
    assert_ne!(*key, 0);
    assert_eq!(v.parts[0].mesh_key, m.content_key());
}

#[test]
fn a_gltf_with_its_data_in_a_uri_and_its_picture_beside_it() {
    use base64::Engine as _;
    let d = Dir::new("gltf");
    let picture = png(2, 2, [10, 20, 30, 255]);
    let t = tri(&[]);
    let data = base64::engine::general_purpose::STANDARD.encode(&t.bin);
    d.write("my pictures/wall.png", &picture);
    let json = tri_json(
        &t,
        json!({ "byteLength": t.bin.len(), "uri": format!("data:application/octet-stream;base64,{data}") }),
        json!({
            "images": [{ "uri": "my%20pictures/wall.png" }],
            "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }]
        }),
    );
    let path = d.write("venue.gltf", serde_json::to_vec(&json).unwrap());
    let v = load_venue(&path, &venue()).unwrap();
    let Paint::Image { rgba, .. } = &v.parts[0].paint else {
        panic!("{:?}", v.parts[0].paint);
    };
    // Plain (a white factor): the picture as it is.
    assert_eq!(&rgba[4..8], &[10, 20, 30, 255]);

    // The picture gone: drawn in its colour, with a warning.
    std::fs::remove_file(d.0.join("my pictures/wall.png")).unwrap();
    let v = load_venue(&path, &venue()).unwrap();
    assert_eq!(v.parts[0].paint, Paint::Colour([1.0, 1.0, 1.0, 1.0]));
    assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
}

#[test]
fn the_venues_brightness_is_in_its_colours_and_pictures() {
    let d = Dir::new("bright");
    let t = tri(&png(2, 1, [100, 100, 100, 255]));
    let json = tri_json(
        &t,
        json!({ "byteLength": t.bin.len() }),
        json!({ "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }] }),
    );
    let path = d.write("venue.glb", glb(&json, &t.bin));
    let mut dim = venue();
    dim.brightness = 0.5;
    let v = load_venue(&path, &dim).unwrap();
    let Paint::Image { rgba, .. } = &v.parts[0].paint else {
        panic!();
    };
    assert!(rgba[4] < 100 && rgba[4] > 50, "{}", rgba[4]);
    assert!(venue().needs_reload(&dim));
    let mut moved = dim.clone();
    moved.placement.position = [1.0, 2.0, 3.0];
    moved.units = LengthUnit::Feet;
    assert!(!moved.needs_reload(&dim));
    assert_eq!(v.loaded_for, dim);
}

#[test]
fn a_picture_over_4096_pixels_is_made_smaller() {
    let d = Dir::new("big");
    let t = tri(&png(MAX_TEXTURE + 100, 2, [1, 2, 3, 255]));
    let json = tri_json(&t, json!({ "byteLength": t.bin.len() }), json!({}));
    let path = d.write("venue.glb", glb(&json, &t.bin));
    let v = load_venue(&path, &venue()).unwrap();
    let Paint::Image { width, height, .. } = &v.parts[0].paint else {
        panic!();
    };
    assert_eq!(*width, MAX_TEXTURE);
    assert!(*height >= 1 && *height <= 2);
    assert!(
        v.warnings.iter().any(|w| w.contains("4196x2")),
        "{:?}",
        v.warnings
    );
}

#[test]
fn a_mirrored_node_keeps_its_triangles_facing_out() {
    let d = Dir::new("mirror");
    let t = tri(&[]);
    let mut json = tri_json(&t, json!({ "byteLength": t.bin.len() }), json!({}));
    json["nodes"][1]["scale"] = json!([-1.0, 1.0, 1.0]);
    json["materials"] = json!([{}]);
    json.as_object_mut().unwrap().remove("textures");
    json.as_object_mut().unwrap().remove("images");
    let path = d.write("venue.glb", glb(&json, &t.bin[..68]));
    let v = load_venue(&path, &venue()).unwrap();
    let m = &v.parts[0].mesh;
    let [a, b, c] = [0, 1, 2].map(|i| m.positions[m.indices[i] as usize]);
    let face = geometry::cross(geometry::sub(b, a), geometry::sub(c, a));
    // Still +Z: the mirror turned the winding round.
    assert!(face[2] > 0.0, "{face:?}");
    // A material with nothing set is white (glTF's default).
    assert_eq!(v.parts[0].paint, Paint::Colour([1.0, 1.0, 1.0, 1.0]));
    assert!(m.normals.iter().all(|n| n[2] > 0.99), "{:?}", m.normals);

    // With the file's own normals (+Z) too: still facing +Z, not inside out.
    let mut bin = t.bin[..68].to_vec();
    bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
    json["buffers"][0]["byteLength"] = json!(bin.len());
    json["bufferViews"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "buffer": 0, "byteOffset": 68, "byteLength": 36 }));
    json["accessors"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC3" }));
    json["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(3);
    let path = d.write("normals.glb", glb(&json, &bin));
    let v = load_venue(&path, &venue()).unwrap();
    let m = &v.parts[0].mesh;
    assert!(
        m.normals.iter().all(|n| close3(*n, [0.0, 0.0, 1.0])),
        "{:?}",
        m.normals
    );
    let [a, b, c] = [0, 1, 2].map(|i| m.positions[m.indices[i] as usize]);
    assert!(geometry::cross(geometry::sub(b, a), geometry::sub(c, a))[2] > 0.0);
}

#[test]
fn glb_data_it_cant_read_is_left_out_or_refused_in_words() {
    let d = Dir::new("types");
    let t = tri(&png(2, 2, [0, 0, 0, 255]));
    let good = tri_json(&t, json!({ "byteLength": t.bin.len() }), json!({}));
    // Positions as shorts (only quantized files may): left out, so nothing.
    let mut j = good.clone();
    j["accessors"][0]["componentType"] = json!(5122);
    let p = d.write("a.glb", glb(&j, &t.bin));
    let e = load_venue(&p, &venue()).unwrap_err();
    assert!(
        e.contains("no triangles") || e.contains("can't read"),
        "{e}"
    );
    // Indices as floats.
    let mut j = good.clone();
    j["accessors"][2]["componentType"] = json!(5126);
    let p = d.write("b.glb", glb(&j, &t.bin));
    assert!(load_venue(&p, &venue()).is_err());
    // Quantized meshes are refused in words.
    let mut j = good.clone();
    j["extensionsUsed"] = json!(["KHR_mesh_quantization"]);
    j["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
    let p = d.write("c.glb", glb(&j, &t.bin));
    let e = load_venue(&p, &venue()).unwrap_err();
    assert!(e.contains("KHR_mesh_quantization"), "{e}");
}

#[test]
fn an_obj_with_a_jpeg_picture() {
    let d = Dir::new("jpeg");
    let img = image::RgbImage::from_pixel(8, 8, image::Rgb([40, 120, 200]));
    let mut jpeg = std::io::Cursor::new(Vec::new());
    img.write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
    d.write("wall.jpg", jpeg.into_inner());
    d.write("wall.mtl", "newmtl w\nmap_Kd wall.jpg\n");
    let obj = "mtllib wall.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\n\
               vt 0 0\nvt 1 0\nvt 0 1\nusemtl w\nf 1/1 2/2 3/3\n";
    let path = d.write("wall.obj", obj);
    let v = load_venue(&path, &venue()).unwrap();
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    let Paint::Image {
        rgba,
        width,
        height,
        ..
    } = &v.parts[0].paint
    else {
        panic!("{:?}", v.parts[0].paint);
    };
    assert_eq!((*width, *height), (8, 8));
    let p = &rgba[0..4];
    let near = |a: u8, b: u8| a.abs_diff(b) <= 4;
    assert!(
        near(p[0], 40) && near(p[1], 120) && near(p[2], 200) && p[3] == 255,
        "{p:?}"
    );
}

#[test]
fn bad_gltf_files_are_errors_not_panics() {
    let d = Dir::new("bad");
    let t = tri(&png(2, 2, [0, 0, 0, 255]));
    let good = tri_json(&t, json!({ "byteLength": t.bin.len() }), json!({}));
    // Not glTF at all.
    let p = d.write("a.glb", b"glTF\x02\x00\x00\x00garbage");
    assert!(load_venue(&p, &venue()).is_err());
    let p = d.write("b.gltf", b"{ not json");
    assert!(load_venue(&p, &venue()).is_err());
    // Cut short: its data is shorter than it says.
    let full = glb(&good, &t.bin);
    let p = d.write("c.glb", &full[..full.len() - 40]);
    assert!(load_venue(&p, &venue()).is_err());
    // An accessor running past its view.
    let mut j = good.clone();
    j["accessors"][0]["count"] = json!(1000);
    let p = d.write("d.glb", glb(&j, &t.bin));
    let e = load_venue(&p, &venue()).unwrap_err();
    assert!(e.contains("damaged") || e.contains("can't read"), "{e}");
    // A view past its buffer.
    let mut j = good.clone();
    j["bufferViews"][0]["byteLength"] = json!(100_000);
    let p = d.write("e.glb", glb(&j, &t.bin));
    assert!(load_venue(&p, &venue()).is_err());
    // Indices past the corners: left out with a warning; nothing else, so an
    // error.
    let mut bin = t.bin.clone();
    bin[60] = 9;
    let p = d.write("f.glb", glb(&good, &bin));
    let e = load_venue(&p, &venue()).unwrap_err();
    assert!(e.contains("no triangles"), "{e}");
    // Draco.
    let mut j = good.clone();
    j["extensionsUsed"] = json!(["KHR_draco_mesh_compression"]);
    j["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    let p = d.write("g.glb", glb(&j, &t.bin));
    let e = load_venue(&p, &venue()).unwrap_err();
    assert!(e.contains("compress"), "{e}");
    // A damaged picture: drawn in colour, with a warning.
    let mut bin = t.bin[..68].to_vec();
    bin.extend_from_slice(b"not a png at all");
    let j = tri_json(
        &Tri {
            bin: bin.clone(),
            png_len: 16,
        },
        json!({ "byteLength": bin.len() }),
        json!({}),
    );
    let p = d.write("h.glb", glb(&j, &bin));
    let v = load_venue(&p, &venue()).unwrap();
    assert!(matches!(v.parts[0].paint, Paint::Colour(_)));
    assert_eq!(v.warnings.len(), 1);
    // Missing altogether.
    assert!(load_venue(&d.0.join("nothing.glb"), &venue()).is_err());
}

#[test]
fn fbx_and_other_formats_are_refused_in_words() {
    let d = Dir::new("fbx");
    let p = d.write("arena.FBX", b"Kaydara FBX Binary  \x00\x1a\x00");
    assert_eq!(load_venue(&p, &venue()).unwrap_err(), FBX_WORDS);
    // Known by its first bytes, whatever its name.
    let p = d.write("arena.model", b"Kaydara FBX Binary  \x00\x1a\x00");
    assert_eq!(load_venue(&p, &venue()).unwrap_err(), FBX_WORDS);
    let p = d.write("arena.3ds", b"MM\x00\x00");
    let e = load_venue(&p, &venue()).unwrap_err();
    assert!(e.contains(FORMAT_WORDS), "{e}");
}

const OBJ: &str = "\
mtllib room.mtl
o Floor
v 0 0 0
v 100 0 0
v 100 0 -100
v 0 0 -100
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vn 0 1 0
usemtl wood
f 1/1/1 2/2/1 3/3/1 4/4/1
o Wall
v 0 0 0
v 0 100 0
v 0 100 100
usemtl red
f 5 7 6
g Pillar
usemtl red
f 5 6 7
";

const MTL: &str = "\
newmtl wood
Kd 0.8 0.8 0.8
map_Kd textures\\wood floor.png
newmtl red
Kd 1 0 0
d 0.5
";

#[test]
fn an_obj_gives_its_groups_by_material_with_colours_and_pictures() {
    let d = Dir::new("obj");
    d.write("textures/wood floor.png", png(2, 2, [50, 60, 70, 255]));
    d.write("room.mtl", MTL);
    let path = d.write("room.obj", OBJ);
    let v = load_venue(&path, &venue()).unwrap();
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    // The quad made two triangles; the wall and the pillar share a material.
    assert_eq!(v.triangles, 4);
    assert_eq!(v.parts.len(), 2);
    let floor = v
        .parts
        .iter()
        .find(|p| matches!(p.paint, Paint::Image { .. }))
        .unwrap();
    assert_eq!(floor.mesh.triangles(), 2);
    assert!(
        floor
            .mesh
            .normals
            .iter()
            .all(|n| close3(*n, [0.0, 1.0, 0.0]))
    );
    // OBJ's v counts up from the bottom; ours down from the top.
    let corner = floor
        .mesh
        .positions
        .iter()
        .position(|p| close3(*p, [0.0, 0.0, -100.0]))
        .unwrap();
    assert_eq!(floor.mesh.uvs[corner], [0.0, 0.0]);
    let Paint::Image { rgba, .. } = &floor.paint else {
        panic!()
    };
    // The picture as it is (its Kd is grey left over).
    assert_eq!(&rgba[4..8], &[50, 60, 70, 255]);
    let red = v
        .parts
        .iter()
        .find(|p| matches!(p.paint, Paint::Colour(_)))
        .unwrap();
    assert_eq!(red.paint, Paint::Colour([1.0, 0.0, 0.0, 0.5]));
    assert_eq!(red.mesh.triangles(), 2);
    let b = v.bounds.unwrap();
    assert!(close3(b.min, [0.0, 0.0, -100.0]) && close3(b.max, [100.0, 100.0, 100.0]));
}

#[test]
fn an_obj_in_centimetres_and_z_up_comes_out_in_metres_y_up() {
    let d = Dir::new("units");
    // A post 100 units tall along Z.
    let path = d.write("post.obj", "v 0 0 0\nv 10 0 0\nv 0 0 100\nf 1 2 3\n");
    let mut v = Venue::new(path.clone());
    v.units = LengthUnit::Centimetres;
    v.up = UpAxis::Z;
    let loaded = load_venue(&path, &v).unwrap();
    // No materials file: grey, no warning.
    assert!(loaded.warnings.is_empty());
    assert_eq!(loaded.parts[0].paint, Paint::Colour(DEFAULT_COLOUR));
    let stage = Stage3d {
        venue: Some(v),
        ..Stage3d::default()
    };
    let show = crate::show::Show::default();
    let scene = stage.scene(&show, Some(&loaded));
    assert_eq!(scene.parts.len(), 1);
    let part = &scene.parts[0];
    assert!(part.lit);
    let m = &part.transform;
    // 100 cm up Z is 1 m up Y; 10 cm along X stays along X.
    assert!(close3(
        geometry::point(m, [0.0, 0.0, 100.0]),
        [0.0, 1.0, 0.0]
    ));
    assert!(close3(
        geometry::point(m, [10.0, 0.0, 0.0]),
        [0.1, 0.0, 0.0]
    ));
    // Z up's -Y (its front) faces the audience, +Z.
    assert!(close3(
        geometry::direction(m, [0.0, -1.0, 0.0]),
        [0.0, 0.0, 0.01]
    ));
    assert!(geometry::det3(m) > 0.0);
    let b = stage.venue_bounds(&loaded).unwrap();
    assert!(
        close3(b.max, [0.1, 1.0, 0.0]) && close3(b.min, [0.0, 0.0, 0.0]),
        "{b:?}"
    );
}

#[test]
fn an_obj_missing_its_materials_or_pictures_loads_with_warnings() {
    let d = Dir::new("obj-missing");
    let path = d.write("room.obj", OBJ);
    let v = load_venue(&path, &venue()).unwrap();
    assert_eq!(v.triangles, 4);
    assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
    assert!(
        v.parts
            .iter()
            .all(|p| p.paint == Paint::Colour(DEFAULT_COLOUR))
    );
    // The materials, but not the picture.
    d.write("room.mtl", MTL);
    let v = load_venue(&path, &venue()).unwrap();
    assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
    assert!(v.warnings[0].contains("wood floor.png"));
    assert!(
        v.parts
            .iter()
            .any(|p| p.paint == Paint::Colour([0.8, 0.8, 0.8, 1.0]))
    );
}

#[test]
fn bad_obj_files_are_errors_not_panics() {
    let d = Dir::new("obj-bad");
    let p = d.write("empty.obj", "# nothing\n");
    assert!(
        load_venue(&p, &venue())
            .unwrap_err()
            .contains("no triangles")
    );
    let p = d.write("broken.obj", "v 0 0 0\nv a b c\nf 1 2 3\n");
    assert!(load_venue(&p, &venue()).is_err());
    let p = d.write("past.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n");
    assert!(load_venue(&p, &venue()).is_err());
    let p = d.write("binary.obj", [0u8, 159, 146, 150, 255, 0, 1]);
    assert!(load_venue(&p, &venue()).is_err());
}

#[test]
fn a_huge_model_is_cut_with_a_warning_and_a_big_one_warned_about() {
    let tri = || Mesh {
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: vec![0, 1, 2],
    };
    let look = || Look {
        colour: DEFAULT_COLOUR,
        texture: None,
    };
    let mut b = Builder::new(1.0);
    (b.limit, b.warn_at) = (10, 5);
    for _ in 0..6 {
        b.add("a", look, tri(), &crate::stage3d::scene::IDENTITY);
    }
    let v = b.finish("big", &venue()).unwrap();
    assert_eq!(v.triangles, 6);
    assert!(v.warnings[0].contains("may slow"), "{:?}", v.warnings);
    let mut b = Builder::new(1.0);
    (b.limit, b.warn_at) = (10, 5);
    for _ in 0..12 {
        b.add("a", look, tri(), &crate::stage3d::scene::IDENTITY);
    }
    let v = b.finish("huge", &venue()).unwrap();
    assert_eq!(v.triangles, 10);
    assert!(
        v.warnings[0].contains("12 triangles; only the first 10"),
        "{:?}",
        v.warnings
    );
    // One piece bigger than the limit: as much of it as fits.
    let mut b = Builder::new(1.0);
    (b.limit, b.warn_at) = (10, 5);
    let mut scan = tri();
    scan.indices = [0, 1, 2].repeat(25);
    b.add("a", look, scan, &crate::stage3d::scene::IDENTITY);
    let v = b.finish("scan", &venue()).unwrap();
    assert_eq!((v.triangles, v.parts[0].mesh.triangles()), (10, 10));
    assert!(
        v.warnings[0].contains("25 triangles; only the first 10"),
        "{:?}",
        v.warnings
    );
    assert_eq!(group_digits(8_000_000), "8,000,000");
    assert_eq!(group_digits(999), "999");
}

#[test]
fn texture_names_from_other_systems_are_found() {
    let d = Dir::new("names");
    let model = d.0.join("room.obj");
    let here = |p: &Path| Found::Here(p.to_path_buf());
    d.write("maps/brick.jpg", b"x");
    assert_eq!(
        texture_file(&model, "maps\\brick.jpg", &here),
        Found::Here(d.0.join("maps/brick.jpg"))
    );
    assert_eq!(
        texture_file(&model, "-bm 0.5 maps/brick.jpg", &here),
        Found::Here(d.0.join("maps/brick.jpg"))
    );
    assert_eq!(
        texture_file(&model, "C:\\Users\\me\\maps\\brick.jpg", &here),
        Found::Missing
    );
    d.write("brick.jpg", b"x");
    assert_eq!(
        texture_file(&model, "C:\\Users\\me\\maps\\brick.jpg", &here),
        Found::Here(d.0.join("brick.jpg"))
    );
    assert_eq!(percent_decode("a%20b%zz%2"), "a b%zz%2");
}

#[test]
fn side_files_stay_in_the_models_folder() {
    let model = Path::new("/shows/venue/room.gltf");
    let side = |n: &str| side_path(model, n).map(|p| p.to_string_lossy().into_owned());
    assert_eq!(side("wall.png").as_deref(), Some("/shows/venue/wall.png"));
    assert_eq!(
        side("maps\\./brick.png").as_deref(),
        Some("/shows/venue/maps/brick.png")
    );
    for refused in [
        "",
        "../secret.bin",
        "maps/../../x.png",
        "/etc/passwd",
        "\\\\server\\share\\x.png",
        "//server/share/x.png",
        "C:\\Windows\\x.png",
        "c:x.png",
        "file:///etc/passwd",
        "https://example.com/x.png",
    ] {
        assert_eq!(side(refused), None, "{refused}");
    }
    // A Windows primary's model, named on any computer.
    assert_eq!(
        side_path(Path::new("C:\\Shows\\room.obj"), "maps/brick.png")
            .unwrap()
            .to_string_lossy(),
        "C:\\Shows\\maps\\brick.png"
    );
}

/// A triangle's glTF with no picture.
fn no_picture() -> serde_json::Value {
    json!({ "images": [], "textures": [], "materials": [{ "pbrMetallicRoughness": {} }] })
}

#[test]
fn a_model_reads_only_files_beside_it_and_no_more_than_it_says() {
    let d = Dir::new("escape");
    let t = tri(&[]);
    // Data beside the folder, named with `..`: refused.
    let outside = Dir::new("escape-outside");
    outside.write("data.bin", &t.bin);
    let name = format!(
        "../{}/data.bin",
        outside.0.file_name().unwrap().to_string_lossy()
    );
    let json = tri_json(
        &t,
        json!({ "byteLength": t.bin.len(), "uri": name }),
        no_picture(),
    );
    let path = d.write("venue.gltf", serde_json::to_vec(&json).unwrap());
    let e = load_venue(&path, &venue()).unwrap_err();
    assert!(e.contains("outside the model's folder"), "{e}");
    // Absolute: refused too.
    let abs = outside.0.join("data.bin").to_string_lossy().into_owned();
    let json = tri_json(
        &t,
        json!({ "byteLength": t.bin.len(), "uri": abs }),
        no_picture(),
    );
    let path = d.write("venue.gltf", serde_json::to_vec(&json).unwrap());
    assert!(load_venue(&path, &venue()).is_err());
    // Not a file (a folder): refused, not read for ever.
    std::fs::create_dir_all(d.0.join("data.bin")).unwrap();
    let json = tri_json(
        &t,
        json!({ "byteLength": t.bin.len(), "uri": "data.bin" }),
        no_picture(),
    );
    let path = d.write("venue.gltf", serde_json::to_vec(&json).unwrap());
    let e = load_venue(&path, &venue()).unwrap_err();
    assert!(e.contains("isn't a file"), "{e}");
    std::fs::remove_dir(d.0.join("data.bin")).unwrap();
    // A buffer file longer than the model says: only what it says is read.
    let mut long = t.bin.clone();
    long.extend(std::iter::repeat_n(7u8, 4096));
    d.write("data.bin", &long);
    let v = load_venue(&path, &venue()).unwrap();
    assert_eq!(v.triangles, 1);
    #[cfg(unix)]
    {
        // A pipe beside it, named as the model's picture: not waited on.
        let fifo = d.0.join("wall.png");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            let json = tri_json(
                &t,
                json!({ "byteLength": t.bin.len(), "uri": "data.bin" }),
                json!({
                    "images": [{ "uri": "wall.png" }],
                    "materials": [{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }]
                }),
            );
            let path = d.write("venue.gltf", serde_json::to_vec(&json).unwrap());
            let v = load_venue(&path, &venue()).unwrap();
            assert!(v.warnings[0].contains("isn't a file"), "{:?}", v.warnings);
        }
    }
    // An OBJ whose materials file is outside its folder: drawn in grey.
    d.write(
        "room.obj",
        OBJ.replace("mtllib room.mtl", "mtllib ../room.mtl"),
    );
    let v = load_venue(&d.0.join("room.obj"), &venue()).unwrap();
    assert!(
        v.warnings[0].contains("outside the model's folder"),
        "{:?}",
        v.warnings
    );
}

#[test]
fn the_files_a_model_reads_are_listed_for_the_network() {
    let d = Dir::new("files");
    // A glTF: its buffer and its picture, not what is inside it.
    let t = tri(&[]);
    let json = tri_json(
        &t,
        json!({ "byteLength": t.bin.len(), "uri": "data/venue%20data.bin" }),
        json!({
            "images": [{ "uri": "wall.png" }, { "uri": "data:image/png;base64,AAAA" }, { "uri": "../x.png" }],
        }),
    );
    let gltf = d.write("venue.gltf", serde_json::to_vec(&json).unwrap());
    assert_eq!(
        venue_files(&gltf),
        vec![d.0.join("data/venue data.bin"), d.0.join("wall.png")]
    );
    // A GLB: only its JSON is read.
    let mut j = tri_json(&t, json!({ "byteLength": t.bin.len() }), json!({}));
    j["images"] = json!([{ "uri": "maps/wall.jpg" }]);
    let glb_path = d.write("venue.glb", glb(&j, &t.bin));
    assert_eq!(venue_files(&glb_path), vec![d.0.join("maps/wall.jpg")]);
    // An OBJ: its materials file and the pictures it names (the one that is
    // there, or the first looked for when none is).
    d.write("room.mtl", MTL);
    let obj = d.write("room.obj", OBJ);
    assert_eq!(
        venue_files(&obj),
        vec![d.0.join("room.mtl"), d.0.join("textures/wood floor.png")]
    );
    d.write("wood floor.png", png(2, 2, [1, 2, 3, 255]));
    assert_eq!(
        venue_files(&obj),
        vec![d.0.join("room.mtl"), d.0.join("wood floor.png")]
    );
    assert!(is_model_file(&obj) && is_model_file(&glb_path) && is_model_file(&gltf));
    assert!(!is_model_file(&d.0.join("room.mtl")));
    assert!(venue_files(&d.0.join("nothing.obj")).is_empty());
}

#[test]
fn a_venue_waits_for_its_files_on_a_show_networks_computer() {
    let d = Dir::new("arriving");
    d.write("textures/wood floor.png", png(2, 2, [50, 60, 70, 255]));
    d.write("room.mtl", MTL);
    let path = d.write("room.obj", OBJ);
    // The model itself still coming.
    let none = |_: &Path| Found::Arriving;
    assert_eq!(
        load_venue_with(&path, &venue(), &none).unwrap_err(),
        Unread::Arriving
    );
    // The model here (at another path, a store's), its picture coming.
    let store = Dir::new("arriving-store");
    let local = store.write("ab/0123.obj", OBJ);
    let mtl = store.write("cd/4567.mtl", MTL);
    let names = (path.clone(), d.0.join("room.mtl"));
    let picture = d.0.join("textures/wood floor.png");
    let coming = |p: &Path| {
        if p == names.0 {
            Found::Here(local.clone())
        } else if p == names.1 {
            Found::Here(mtl.clone())
        } else if p == picture {
            Found::Arriving
        } else {
            // Names not in the primary's list: on their way until it is.
            Found::Arriving
        }
    };
    assert_eq!(
        load_venue_with(&path, &venue(), &coming).unwrap_err(),
        Unread::Arriving
    );
    // Come: read from the store's copies.
    let pic = store.write("ef/89ab.png", png(2, 2, [50, 60, 70, 255]));
    let come = |p: &Path| {
        if p == picture {
            Found::Here(pic.clone())
        } else {
            coming(p)
        }
    };
    let v = load_venue_with(&path, &venue(), &come).unwrap();
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    assert!(
        v.parts
            .iter()
            .any(|p| matches!(p.paint, Paint::Image { .. }))
    );
    // One the primary hadn't either: drawn in colour, not waited for.
    let gone = |p: &Path| {
        if p == picture {
            Found::Missing
        } else {
            coming(p)
        }
    };
    let v = load_venue_with(&path, &venue(), &gone).unwrap();
    assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
}

#[test]
fn a_part_too_big_for_the_graphics_card_is_cut_into_pieces() {
    // Flat-shaded triangles: three corners each, none shared.
    let mut scan = Mesh::default();
    for k in 0..10u32 {
        let x = k as f32;
        scan.positions
            .extend([[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]]);
        scan.indices.extend([3 * k, 3 * k + 1, 3 * k + 2]);
    }
    let look = || Look {
        colour: DEFAULT_COLOUR,
        texture: None,
    };
    let mut b = Builder::new(1.0);
    b.part_vertices = 7;
    b.add("a", look, scan.clone(), &crate::stage3d::scene::IDENTITY);
    let v = b.finish("scan", &venue()).unwrap();
    // 30 corners in pieces of at most 7 (whole triangles): 6, 6, 6, 6, 6.
    assert_eq!(v.parts.len(), 5);
    for p in &v.parts {
        assert!(p.mesh.positions.len() <= 7);
        assert_eq!(p.mesh.normals.len(), p.mesh.positions.len());
        assert_eq!(p.mesh.uvs.len(), p.mesh.positions.len());
        assert!(
            p.mesh
                .indices
                .iter()
                .all(|i| (*i as usize) < p.mesh.positions.len())
        );
    }
    assert_eq!(
        v.parts.iter().map(|p| p.mesh.triangles()).sum::<usize>(),
        10
    );
    let b = v.bounds.unwrap();
    assert!(close3(b.min, [0.0; 3]) && close3(b.max, [10.0, 1.0, 0.0]));
    // Cut at the triangle limit: the corners of the triangles left out go
    // too (and from the bounds).
    let mut b = Builder::new(1.0);
    (b.limit, b.warn_at) = (4, 2);
    b.add("a", look, scan, &crate::stage3d::scene::IDENTITY);
    let v = b.finish("scan", &venue()).unwrap();
    assert_eq!(v.parts.len(), 1);
    assert_eq!(v.parts[0].mesh.positions.len(), 12);
    let b = v.bounds.unwrap();
    assert!(close3(b.max, [4.0, 1.0, 0.0]), "{b:?}");
    // A mesh that fits and uses every corner is kept as it is.
    let m = Mesh {
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0; 2]; 3],
        indices: vec![0, 1, 2],
    };
    assert_eq!(split_mesh(m.clone(), 7), vec![m]);
}

#[test]
fn the_loaded_venue_can_go_to_another_thread() {
    fn send<T: Send + 'static>() {}
    send::<LoadedVenue>();
    send::<Result<LoadedVenue, String>>();
}
