//! Writing meshes as a binary glTF (.glb), pure: so a sample venue can be
//! taken into Blender (or any 3D program) and changed. One mesh, node and
//! material per part; positions, normals and indices; plain colours
//! (`baseColorFactor`, linear, not metallic). Read back by `load::load_venue`.

use super::scene::Mesh;
use serde_json::json;

/// One part to write: its name, its linear RGBA colour and its triangles.
pub struct GlbPart<'a> {
    pub name: &'a str,
    pub colour: [f32; 4],
    pub mesh: &'a Mesh,
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;

/// The parts as a .glb file's bytes.
pub fn write(parts: &[GlbPart]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors, mut meshes, mut materials, mut nodes) =
        (vec![], vec![], vec![], vec![], vec![]);
    for (i, p) in parts.iter().enumerate() {
        let m = p.mesh;
        if m.indices.is_empty() || m.positions.is_empty() {
            continue;
        }
        let mut view = |bytes: &[u8], target: u32| {
            // Every piece is 4-byte values, so offsets stay aligned.
            views.push(json!({
                "buffer": 0,
                "byteOffset": bin.len(),
                "byteLength": bytes.len(),
                "target": target,
            }));
            bin.extend_from_slice(bytes);
            views.len() - 1
        };
        let floats = |v: &[[f32; 3]]| -> Vec<u8> {
            v.iter()
                .flat_map(|p| p.iter().flat_map(|c| c.to_le_bytes()))
                .collect()
        };
        let pos = view(&floats(&m.positions), ARRAY_BUFFER);
        let normals =
            (m.normals.len() == m.positions.len()).then(|| view(&floats(&m.normals), ARRAY_BUFFER));
        let idx: Vec<u8> = m.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let idx = view(&idx, ELEMENT_ARRAY_BUFFER);

        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &m.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let mut attributes = serde_json::Map::new();
        accessors.push(json!({
            "bufferView": pos, "componentType": FLOAT, "count": m.positions.len(),
            "type": "VEC3", "min": min, "max": max,
        }));
        attributes.insert("POSITION".into(), json!(accessors.len() - 1));
        if let Some(n) = normals {
            accessors.push(json!({
                "bufferView": n, "componentType": FLOAT, "count": m.normals.len(),
                "type": "VEC3",
            }));
            attributes.insert("NORMAL".into(), json!(accessors.len() - 1));
        }
        accessors.push(json!({
            "bufferView": idx, "componentType": UNSIGNED_INT, "count": m.indices.len(),
            "type": "SCALAR",
        }));
        let indices = accessors.len() - 1;
        materials.push(json!({
            "name": p.name,
            "pbrMetallicRoughness": {
                "baseColorFactor": p.colour.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 1.0 }),
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
        }));
        meshes.push(json!({
            "name": p.name,
            "primitives": [{
                "attributes": attributes,
                "indices": indices,
                "material": materials.len() - 1,
            }],
        }));
        nodes.push(json!({ "name": format!("{} {}", p.name, i + 1), "mesh": meshes.len() - 1 }));
    }
    let node_ids: Vec<usize> = (0..nodes.len()).collect();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "NewQ" },
        "scene": 0,
        "scenes": [{ "nodes": node_ids }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
    });
    if !bin.is_empty() {
        doc["buffers"] = json!([{ "byteLength": bin.len() }]);
    }
    let mut text = serde_json::to_vec(&doc).unwrap_or_default();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + text.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
    }
    out
}
