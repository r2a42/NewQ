//! Reading a venue model: glTF/GLB (`gltf`) and OBJ with its MTL (`tobj`).
//! Slow (seconds for a big model): call `load_venue` on a worker thread, never
//! the UI's; what it gives back is `Send`.
//!
//! What comes out is in the file's own coordinates and units, with the
//! file's node transforms already applied to the positions; `Venue::matrix`
//! (units, up axis, placement) is the part's transform in `Stage3d::scene`.
//! The parts are grouped by material, one part per material. Colours and
//! pictures already carry the venue's brightness.

// The Preview pane's 3D mode loads venues through this (integration).
#![allow(dead_code)]

use super::Venue;
use super::geometry::{self, Mat4};
use super::scene::{Fnv, Mesh, Paint};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Over this many triangles a model is still drawn, with a warning that it
/// may slow the computer down.
pub const WARN_TRIANGLES: usize = 2_000_000;
/// Triangles past this many are left out (with a warning).
pub const MAX_TRIANGLES: usize = 8_000_000;
/// A picture wider or taller than this is made smaller to fit.
pub const MAX_TEXTURE: u32 = 4096;
/// What a part with no colour of its own is painted (a light grey).
pub const DEFAULT_COLOUR: [f32; 4] = [0.7, 0.7, 0.7, 1.0];

/// What `load_venue` says to a model in a format it doesn't read.
pub const FBX_WORDS: &str = "NewQ can't read FBX models yet. Export the model as glTF (.glb) \
     from your 3D program (Blender, 3ds Max, Maya, Cinema 4D and SketchUp all can), or as OBJ.";
pub const FORMAT_WORDS: &str = "NewQ reads venue models as glTF (.glb or .gltf) or OBJ (.obj).";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub fn with(self, p: [f32; 3]) -> Bounds {
        Bounds {
            min: [0, 1, 2].map(|i| self.min[i].min(p[i])),
            max: [0, 1, 2].map(|i| self.max[i].max(p[i])),
        }
    }

    pub fn size(&self) -> [f32; 3] {
        [0, 1, 2].map(|i| self.max[i] - self.min[i])
    }
}

/// One material's triangles.
#[derive(Clone, Debug)]
pub struct VenuePart {
    pub mesh: Arc<Mesh>,
    pub mesh_key: u64,
    pub paint: Paint,
}

/// A venue model, read.
#[derive(Clone, Debug)]
pub struct LoadedVenue {
    pub parts: Vec<VenuePart>,
    /// Around every position, in the file's coordinates (None: nothing in
    /// it).
    pub bounds: Option<Bounds>,
    pub triangles: usize,
    /// What the user should know: a missing texture or materials file, a
    /// model so big it may be slow, parts left out.
    pub warnings: Vec<String>,
    /// The venue as it was when this was read (`Venue::needs_reload`).
    pub loaded_for: Venue,
}

/// Read the venue's model from `path` (the venue's own, resolved). An error
/// is a sentence for the user; a model that reads with something missing
/// (a texture, its materials) loads with `warnings`. Never panics on a bad
/// file.
pub fn load_venue(path: &Path, venue: &Venue) -> Result<LoadedVenue, String> {
    // The readers are trusted not to panic, but a damaged file must never
    // take the program down.
    let read = std::panic::catch_unwind(|| read_model(path, venue));
    read.unwrap_or_else(|_| {
        Err(format!(
            "{} looks damaged: it couldn't be read.",
            file_name(path)
        ))
    })
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Gltf,
    Obj,
    Fbx,
}

fn format_of(path: &Path, head: &[u8]) -> Option<Format> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("glb" | "gltf") => return Some(Format::Gltf),
        Some("obj") => return Some(Format::Obj),
        Some("fbx") => return Some(Format::Fbx),
        _ => {}
    }
    if head.starts_with(b"glTF") {
        Some(Format::Gltf)
    } else if head.starts_with(b"Kaydara FBX Binary") || head.starts_with(b"; FBX") {
        Some(Format::Fbx)
    } else {
        None
    }
}

fn read_model(path: &Path, venue: &Venue) -> Result<LoadedVenue, String> {
    let name = file_name(path);
    let opened = |e: std::io::Error| format!("{name} can't be opened: {e}");
    let mut head = Vec::with_capacity(32);
    {
        use std::io::Read as _;
        std::fs::File::open(path)
            .and_then(|f| f.take(32).read_to_end(&mut head))
            .map_err(opened)?;
    }
    let mut b = Builder::new(venue.brightness());
    match format_of(path, &head) {
        Some(Format::Gltf) => {
            let bytes = std::fs::read(path).map_err(opened)?;
            read_gltf(path, &bytes, &mut b)?
        }
        Some(Format::Obj) => read_obj(path, &mut b)?,
        Some(Format::Fbx) => return Err(FBX_WORDS.into()),
        None => return Err(format!("{name}: {FORMAT_WORDS}")),
    }
    b.finish(&name, venue)
}

// ----- Building the parts ------------------------------------------------------

/// A material, as far as the venue view cares.
#[derive(Clone, Debug)]
struct Look {
    colour: [f32; 4],
    texture: Option<Arc<image::RgbaImage>>,
}

struct Builder {
    brightness: f32,
    /// Per material key, its look and its triangles so far.
    parts: Vec<(Look, Mesh)>,
    by_key: HashMap<String, usize>,
    triangles: usize,
    left_out: usize,
    warnings: Vec<String>,
    bounds: Option<Bounds>,
    /// `MAX_TRIANGLES` and `WARN_TRIANGLES` (smaller in tests).
    limit: usize,
    warn_at: usize,
}

impl Builder {
    fn new(brightness: f32) -> Self {
        Builder {
            brightness,
            parts: Vec::new(),
            by_key: HashMap::new(),
            triangles: 0,
            left_out: 0,
            warnings: Vec::new(),
            bounds: None,
            limit: MAX_TRIANGLES,
            warn_at: WARN_TRIANGLES,
        }
    }

    fn warn(&mut self, w: String) {
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    /// Add triangles of one material: positions placed by `m`, normals
    /// carried with them (or worked out when there are none), the winding
    /// turned round when `m` mirrors.
    fn add(&mut self, key: &str, look: impl FnOnce() -> Look, mut piece: Mesh, m: &Mat4) {
        let n = piece.positions.len();
        piece.indices.truncate(piece.indices.len() / 3 * 3);
        if piece.indices.iter().any(|i| *i as usize >= n) {
            self.warn(
                "Some of the model's triangles point past its corners; they are left out.".into(),
            );
            return;
        }
        let mut tris = piece.indices.len() / 3;
        if tris == 0 {
            return;
        }
        // Past the limit, as much of it as fits.
        let room = self.limit.saturating_sub(self.triangles);
        if tris > room {
            self.left_out += tris - room;
            tris = room;
            piece.indices.truncate(room * 3);
            if tris == 0 {
                return;
            }
        }
        self.triangles += tris;
        for p in &mut piece.positions {
            *p = geometry::point(m, *p);
            self.bounds = Some(match self.bounds {
                None => Bounds { min: *p, max: *p },
                Some(b) => b.with(*p),
            });
        }
        if geometry::det3(m) < 0.0 {
            for t in piece.indices.chunks_mut(3) {
                t.swap(1, 2);
            }
        }
        if piece.normals.len() == n {
            // The cofactor matrix is the inverse transpose times the
            // determinant: under a mirror it turns normals inside out.
            let nm = geometry::normal_matrix(m);
            let sign = if geometry::det3(m) < 0.0 { -1.0 } else { 1.0 };
            for v in &mut piece.normals {
                let t = geometry::apply3(&nm, *v);
                *v = geometry::normalize(t.map(|c| c * sign));
            }
        } else {
            piece.normals = smooth_normals(&piece.positions, &piece.indices);
        }
        if piece.uvs.len() != n {
            piece.uvs = vec![[0.0, 0.0]; n];
        }
        let at = match self.by_key.get(key) {
            Some(i) => *i,
            None => {
                self.parts.push((look(), Mesh::default()));
                self.by_key.insert(key.to_string(), self.parts.len() - 1);
                self.parts.len() - 1
            }
        };
        let mesh = &mut self.parts[at].1;
        let base = mesh.positions.len() as u32;
        mesh.positions.extend(piece.positions);
        mesh.normals.extend(piece.normals);
        mesh.uvs.extend(piece.uvs);
        mesh.indices.extend(piece.indices.iter().map(|i| i + base));
    }

    fn finish(mut self, name: &str, venue: &Venue) -> Result<LoadedVenue, String> {
        if self.triangles == 0 {
            return Err(format!("{name} has no triangles to show."));
        }
        if self.left_out > 0 {
            self.warn(format!(
                "The model has {} triangles; only the first {} are shown. Simplify it in \
                 your 3D program (a decimate modifier) for the whole of it.",
                group_digits(self.triangles + self.left_out),
                group_digits(self.limit)
            ));
        } else if self.triangles > self.warn_at {
            self.warn(format!(
                "The model has {} triangles, which may slow the 3D view down. Over {} is a \
                 lot: simplify it in your 3D program if the view is slow.",
                group_digits(self.triangles),
                group_digits(self.warn_at)
            ));
        }
        let brightness = self.brightness;
        let mut pictures: HashMap<usize, (Arc<Vec<u8>>, u64)> = HashMap::new();
        let parts = self
            .parts
            .into_iter()
            .filter(|(_, m)| !m.indices.is_empty())
            .map(|(look, mesh)| {
                let paint = paint_of(&look, brightness, &mut pictures);
                let mesh_key = mesh.content_key();
                VenuePart {
                    mesh: Arc::new(mesh),
                    mesh_key,
                    paint,
                }
            })
            .collect();
        Ok(LoadedVenue {
            parts,
            bounds: self.bounds,
            triangles: self.triangles,
            warnings: self.warnings,
            loaded_for: venue.clone(),
        })
    }
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A material's paint: its colour, or its picture with the colour and the
/// brightness multiplied in. One picture used plainly by several materials
/// is shared.
fn paint_of(
    look: &Look,
    brightness: f32,
    pictures: &mut HashMap<usize, (Arc<Vec<u8>>, u64)>,
) -> Paint {
    let [r, g, b, a] = look.colour;
    let Some(tex) = &look.texture else {
        return Paint::Colour([r * brightness, g * brightness, b * brightness, a]);
    };
    let factor = [r * brightness, g * brightness, b * brightness];
    let plain = factor == [1.0; 3] && a == 1.0;
    let (w, h) = tex.dimensions();
    let id = Arc::as_ptr(tex) as usize;
    let made = |rgba: Vec<u8>| {
        let mut k = Fnv::new();
        k.bytes(&w.to_le_bytes());
        k.bytes(&h.to_le_bytes());
        k.bytes(&rgba);
        (Arc::new(rgba), k.0)
    };
    let (rgba, key) = if plain {
        pictures
            .entry(id)
            .or_insert_with(|| made(tex.as_raw().clone()))
            .clone()
    } else {
        // The pixels are sRGB and the factor linear: near enough, multiply
        // the sRGB values by the factor's sRGB equivalent.
        let f = factor.map(|v| v.max(0.0).powf(1.0 / 2.2));
        let mut px = tex.as_raw().clone();
        for p in px.chunks_mut(4) {
            for c in 0..3 {
                p[c] = (f32::from(p[c]) * f[c]).round().min(255.0) as u8;
            }
            p[3] = (f32::from(p[3]) * a.clamp(0.0, 1.0)).round() as u8;
        }
        made(px)
    };
    Paint::Image {
        rgba,
        width: w,
        height: h,
        key,
    }
}

/// Each corner's normal: the area-weighted sum of its triangles'.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut sums = vec![[0.0f32; 3]; positions.len()];
    for t in indices.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|i| positions[t[i] as usize]);
        let face = geometry::cross(geometry::sub(b, a), geometry::sub(c, a));
        for i in t {
            let s = &mut sums[*i as usize];
            for k in 0..3 {
                s[k] += face[k];
            }
        }
    }
    sums.into_iter().map(geometry::normalize).collect()
}

/// A picture from a file's bytes, made to fit `MAX_TEXTURE`.
fn decode_picture(bytes: &[u8], what: &str, b: &mut Builder) -> Option<Arc<image::RgbaImage>> {
    let img = match image::load_from_memory(bytes) {
        Ok(i) => i,
        Err(e) => {
            b.warn(format!(
                "The picture {what} can't be read ({e}); its part is drawn in colour."
            ));
            return None;
        }
    };
    let (w, h) = (img.width(), img.height());
    let img = if w > MAX_TEXTURE || h > MAX_TEXTURE {
        b.warn(format!(
            "The picture {what} is {w}x{h}; it is drawn at most {MAX_TEXTURE} pixels across."
        ));
        img.resize(
            MAX_TEXTURE,
            MAX_TEXTURE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    Some(Arc::new(img.to_rgba8()))
}

// ----- glTF --------------------------------------------------------------------

/// Extensions a file may require that we can still draw it without.
const GLTF_IGNORABLE: &[&str] = &[
    "KHR_materials_unlit",
    "KHR_texture_transform",
    "KHR_materials_emissive_strength",
    "KHR_materials_ior",
    "KHR_materials_specular",
    "KHR_materials_transmission",
    "KHR_materials_volume",
    "KHR_materials_clearcoat",
    "KHR_materials_sheen",
    "KHR_materials_variants",
    "KHR_lights_punctual",
];

fn read_gltf(path: &Path, bytes: &[u8], b: &mut Builder) -> Result<(), String> {
    use gltf::accessor::{DataType, Dimensions};
    let name = file_name(path);
    let mut gltf = gltf::Gltf::from_slice(bytes)
        .map_err(|e| format!("{name} isn't a glTF file NewQ can read ({e})."))?;
    let doc = &gltf.document;
    for ext in doc.extensions_required() {
        if ext == "KHR_draco_mesh_compression"
            || ext == "EXT_meshopt_compression"
            || ext == "KHR_mesh_quantization"
        {
            return Err(format!(
                "{name} is compressed ({ext}), which NewQ can't read. Export it again \
                 without mesh compression."
            ));
        }
        if !GLTF_IGNORABLE.contains(&ext) {
            return Err(format!(
                "{name} needs the glTF extension {ext}, which NewQ can't read."
            ));
        }
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in doc.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .take()
                .ok_or_else(|| format!("{name} is missing its binary part."))?,
            gltf::buffer::Source::Uri(uri) => read_uri(uri, dir)
                .map_err(|e| format!("{name}'s data ({}) can't be read: {e}", short(uri)))?,
        };
        if data.len() < buffer.length() {
            return Err(format!(
                "{name}'s data is shorter than the file says: it may be cut short."
            ));
        }
        buffers.push(data);
    }
    // Every view must lie within its buffer, so reading the accessors can't
    // run off the end.
    for view in doc.views() {
        let end = view.offset().checked_add(view.length());
        let len = buffers.get(view.buffer().index()).map(Vec::len);
        if end.is_none() || len.is_none() || end > len {
            return Err(format!(
                "{name} is damaged: its data doesn't fit its buffers."
            ));
        }
    }
    for accessor in doc.accessors() {
        let Some(view) = accessor.view() else {
            continue;
        };
        let stride = view.stride().unwrap_or(accessor.size());
        let need = accessor.count().checked_sub(1).and_then(|n| {
            n.checked_mul(stride)?
                .checked_add(accessor.offset())?
                .checked_add(accessor.size())
        });
        if accessor.count() > 0 && need.is_none_or(|n| n > view.length()) {
            return Err(format!(
                "{name} is damaged: its data doesn't fit its buffers."
            ));
        }
    }

    let mut pictures: HashMap<usize, Option<Arc<image::RgbaImage>>> = HashMap::new();
    let roots: Vec<gltf::Node> = match doc.default_scene().or_else(|| doc.scenes().next()) {
        Some(scene) => scene.nodes().collect(),
        None => {
            // No scene: every node that is no other's child.
            let children: Vec<usize> = doc
                .nodes()
                .flat_map(|n| n.children().map(|c| c.index()).collect::<Vec<_>>())
                .collect();
            doc.nodes()
                .filter(|n| !children.contains(&n.index()))
                .collect()
        }
    };
    let mut stack: Vec<(gltf::Node, Mat4, usize)> = roots
        .into_iter()
        .map(|n| (n, super::scene::IDENTITY, 0))
        .collect();
    let mut skipped_modes = false;
    let mut visited = 0usize;
    while let Some((node, parent, depth)) = stack.pop() {
        visited += 1;
        if depth > 256 || visited > 1_000_000 {
            return Err(format!("{name} is damaged: its nodes go round in a loop."));
        }
        let m = geometry::mul(&parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives() {
                let reader = prim.reader(|buf| buffers.get(buf.index()).map(Vec::as_slice));
                // The reader takes positions and normals as floats, and the
                // rest as glTF allows them: anything else is a damaged file
                // (or quantized, which was refused above).
                let material = prim.material();
                let pbr = material.pbr_metallic_roughness();
                let texture = pbr.base_color_texture();
                let set = texture.as_ref().map_or(0, |t| t.tex_coord());
                let is = |a: &gltf::Accessor, dims: Dimensions, types: &[DataType]| {
                    a.dimensions() == dims && types.contains(&a.data_type())
                };
                let fits = |sem: gltf::Semantic, dims: Dimensions, types: &[DataType]| {
                    prim.get(&sem).is_none_or(|a| is(&a, dims, types))
                };
                let readable =
                    fits(
                        gltf::Semantic::Positions,
                        Dimensions::Vec3,
                        &[DataType::F32],
                    ) && fits(gltf::Semantic::Normals, Dimensions::Vec3, &[DataType::F32])
                        && fits(
                            gltf::Semantic::TexCoords(set),
                            Dimensions::Vec2,
                            &[DataType::U8, DataType::U16, DataType::F32],
                        )
                        && prim.indices().is_none_or(|a| {
                            is(
                                &a,
                                Dimensions::Scalar,
                                &[DataType::U8, DataType::U16, DataType::U32],
                            )
                        });
                if !readable {
                    b.warn(format!(
                        "Part of {name} is stored in a way NewQ can't read; it is left out."
                    ));
                    continue;
                }
                let Some(positions) = reader.read_positions() else {
                    continue;
                };
                let positions: Vec<[f32; 3]> = positions.collect();
                let count = positions.len() as u32;
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..count).collect(),
                };
                let indices = match prim.mode() {
                    gltf::mesh::Mode::Triangles => indices,
                    gltf::mesh::Mode::TriangleStrip => strip(&indices),
                    gltf::mesh::Mode::TriangleFan => fan(&indices),
                    _ => {
                        skipped_modes = true;
                        continue;
                    }
                };
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|n| n.collect())
                    .unwrap_or_default();
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(set)
                    .map(|t| t.into_f32().collect())
                    .unwrap_or_default();
                let key = format!("gltf:{:?}", material.index());
                let image = texture.map(|t| t.texture().source());
                let picture = image.and_then(|img| {
                    pictures
                        .entry(img.index())
                        .or_insert_with(|| gltf_picture(&img, &buffers, dir, b))
                        .clone()
                });
                let colour = pbr.base_color_factor();
                b.add(
                    &key,
                    || Look {
                        colour,
                        texture: picture,
                    },
                    Mesh {
                        positions,
                        normals,
                        uvs,
                        indices,
                    },
                    &m,
                );
            }
        }
        for child in node.children() {
            stack.push((child, m, depth + 1));
        }
    }
    if skipped_modes {
        b.warn("Points and lines in the model are left out: only its surfaces are drawn.".into());
    }
    Ok(())
}

fn gltf_picture(
    img: &gltf::Image,
    buffers: &[Vec<u8>],
    dir: &Path,
    b: &mut Builder,
) -> Option<Arc<image::RgbaImage>> {
    let what = img.name().map_or_else(
        || format!("number {}", img.index() + 1),
        |n| format!("\"{n}\""),
    );
    let bytes = match img.source() {
        gltf::image::Source::View { view, .. } => {
            let data = buffers.get(view.buffer().index())?;
            data.get(view.offset()..view.offset() + view.length())?
                .to_vec()
        }
        gltf::image::Source::Uri { uri, .. } => match read_uri(uri, dir) {
            Ok(d) => d,
            Err(e) => {
                b.warn(format!(
                    "The picture {} can't be read ({e}); its part is drawn in colour.",
                    short(uri)
                ));
                return None;
            }
        },
    };
    decode_picture(&bytes, &what, b)
}

fn short(uri: &str) -> String {
    if uri.starts_with("data:") {
        "inside the file".into()
    } else {
        uri.chars().take(120).collect()
    }
}

/// A buffer's or a picture's bytes: a `data:` URI's, or a file's beside the
/// model.
fn read_uri(uri: &str, dir: &Path) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    if let Some(rest) = uri.strip_prefix("data:") {
        let (head, data) = rest.split_once(',').ok_or("a data URI without data")?;
        if !head.ends_with(";base64") {
            return Err("a data URI that isn't base64".into());
        }
        return base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| e.to_string());
    }
    let path = dir.join(percent_decode(uri));
    std::fs::read(&path).map_err(|e| e.to_string())
}

/// `%20` and the like, as glTF URIs have them.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        if b[i] == b'%'
            && let Some(hex) = hex.filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A triangle strip's triangles, every other one turned so all keep their
/// winding.
fn strip(i: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    for k in 2..i.len() {
        if k % 2 == 0 {
            out.extend_from_slice(&[i[k - 2], i[k - 1], i[k]]);
        } else {
            out.extend_from_slice(&[i[k - 1], i[k - 2], i[k]]);
        }
    }
    out
}

fn fan(i: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    for k in 2..i.len() {
        out.extend_from_slice(&[i[0], i[k - 1], i[k]]);
    }
    out
}

// ----- OBJ ---------------------------------------------------------------------

fn read_obj(path: &Path, b: &mut Builder) -> Result<(), String> {
    let name = file_name(path);
    let options = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ignore_points: true,
        ignore_lines: true,
    };
    let (models, materials) =
        tobj::load_obj(path, &options).map_err(|e| format!("{name} can't be read as OBJ: {e}."))?;
    let materials = match materials {
        Ok(m) => m,
        Err(e) => {
            b.warn(format!(
                "{name}'s materials file can't be read ({e}); the model is drawn in grey."
            ));
            Vec::new()
        }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut pictures: HashMap<PathBuf, Option<Arc<image::RgbaImage>>> = HashMap::new();
    for model in models {
        let mesh = model.mesh;
        let positions: Vec<[f32; 3]> = mesh.positions.as_chunks::<3>().0.to_vec();
        let normals: Vec<[f32; 3]> = mesh.normals.as_chunks::<3>().0.to_vec();
        // OBJ's v is 0 at the bottom of the picture.
        let uvs: Vec<[f32; 2]> = mesh
            .texcoords
            .as_chunks::<2>()
            .0
            .iter()
            .map(|t| [t[0], 1.0 - t[1]])
            .collect();
        let material = mesh.material_id.and_then(|i| materials.get(i));
        let key = format!("obj:{:?}", mesh.material_id.filter(|_| material.is_some()));
        let look = match material {
            None => Look {
                colour: DEFAULT_COLOUR,
                texture: None,
            },
            Some(mat) => {
                let [r, g, bl] = mat.diffuse.unwrap_or([0.7, 0.7, 0.7]);
                let alpha = mat.dissolve.unwrap_or(1.0);
                let texture = mat.diffuse_texture.as_deref().and_then(|t| {
                    let Some(file) = texture_file(dir, t) else {
                        b.warn(format!(
                            "The picture {t} isn't beside the model; its part is drawn in colour."
                        ));
                        return None;
                    };
                    pictures
                        .entry(file.clone())
                        .or_insert_with(|| match std::fs::read(&file) {
                            Ok(bytes) => decode_picture(&bytes, t, b),
                            Err(e) => {
                                b.warn(format!("The picture {t} can't be read ({e})."));
                                None
                            }
                        })
                        .clone()
                });
                Look {
                    colour: [r, g, bl, alpha],
                    // A texture is shown as it is, as most programs show an
                    // OBJ's (its Kd is often grey left over).
                    texture,
                }
            }
        };
        let look = if look.texture.is_some() {
            Look {
                colour: [1.0, 1.0, 1.0, look.colour[3]],
                ..look
            }
        } else {
            look
        };
        b.add(
            &key,
            || look,
            Mesh {
                positions,
                normals,
                uvs,
                indices: mesh.indices,
            },
            &super::scene::IDENTITY,
        );
    }
    Ok(())
}

/// Where an MTL's `map_Kd` picture is: its name may have Windows' `\`, and
/// options before it (`-bm 1 wall.png`).
fn texture_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim().replace('\\', "/");
    let mut tries = vec![dir.join(&name)];
    if let Some(last) = name.split_whitespace().last() {
        tries.push(dir.join(last));
    }
    if let Some(file) = Path::new(&name).file_name() {
        tries.push(dir.join(file));
    }
    tries.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
