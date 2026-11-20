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
/// A part (one material's triangles) with more corners than this is cut into
/// pieces: the renderer's corners are 32 bytes, and a graphics card may take
/// buffers no bigger than 256 MB (8,388,608 corners).
pub const MAX_PART_VERTICES: usize = 4_000_000;
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
///
/// A sample venue (`Venue::sample`) is made in code instead, and `path` is
/// not looked at.
pub fn load_venue(path: &Path, venue: &Venue) -> Result<LoadedVenue, String> {
    load_venue_with(path, venue, &|p| Found::Here(p.to_path_buf())).map_err(|e| match e {
        Unread::Failed(why) => why,
        Unread::Arriving => format!("{} is still arriving.", file_name(path)),
    })
}

/// Where a file a model names is on this computer: the file itself, or on
/// a show network's follower the store's copy (`net::media::local_path`).
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Here(PathBuf),
    /// On its way from the primary: the model is read once it has come.
    Arriving,
    Missing,
}

/// Why a venue wasn't read.
#[derive(Clone, Debug, PartialEq)]
pub enum Unread {
    /// A sentence for the user.
    Failed(String),
    /// A file it needs is still on its way (a show network's follower).
    Arriving,
}

/// A model's picture file bigger than this is not read.
pub const MAX_PICTURE_FILE: u64 = 256 << 20;
/// An OBJ's materials file bigger than this is not read.
pub const MAX_MATERIALS_FILE: u64 = 64 << 20;

/// `load_venue`, every file found through `locate`: the model's own (`path`
/// as the show names it) and the files it names beside it (`side_path`
/// of their names), which are read only when they are regular files, never
/// past their size, and never outside the model's folder.
pub fn load_venue_with(
    path: &Path,
    venue: &Venue,
    locate: &dyn Fn(&Path) -> Found,
) -> Result<LoadedVenue, Unread> {
    if let Some(kind) = venue.sample {
        return super::samples::sample_venue(kind, venue).map_err(Unread::Failed);
    }
    // The readers are trusted not to panic, but a damaged file must never
    // take the program down.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_model(path, venue, locate)
    }));
    read.unwrap_or_else(|_| {
        Err(Unread::Failed(format!(
            "{} looks damaged: it couldn't be read.",
            file_name(path)
        )))
    })
}

/// Where a file a model names (`name`, as the model writes it) is, beside
/// the model at `model`: put together as text, with the model's own kind
/// of separator (a Windows primary's `C:\…` on any computer). None for a
/// name that would leave the model's folder: absolute, a drive or UNC
/// path, a URL, or one with `..` in it. Subfolders are fine.
pub fn side_path(model: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim();
    let b = name.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || drive
        || name.contains("://")
    {
        return None;
    }
    let parts: Vec<&str> = name
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() || parts.contains(&"..") {
        return None;
    }
    let text = model.to_string_lossy();
    Some(PathBuf::from(match text.rfind(['/', '\\']) {
        Some(i) => {
            let sep = &text[i..=i];
            format!("{}{sep}{}", &text[..i], parts.join(sep))
        }
        None => parts.join(std::path::MAIN_SEPARATOR_STR),
    }))
}

/// A regular file's bytes, at most `most` of them; with `whole`, a longer
/// file is refused rather than cut.
fn read_capped(path: &Path, most: u64, whole: bool) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("it isn't a file".into());
    }
    if whole && meta.len() > most {
        return Err(format!("it is over {} MB", most >> 20));
    }
    let mut out = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(most).read_to_end(&mut out))
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// A file the model at `model` names, read through `locate` (see
/// `read_capped`); one still on its way marks the build as waiting for it.
fn read_side(
    model: &Path,
    name: &str,
    most: u64,
    whole: bool,
    locate: &dyn Fn(&Path) -> Found,
    b: &mut Builder,
) -> Result<Vec<u8>, String> {
    let Some(shown) = side_path(model, name) else {
        return Err("it is outside the model's folder".into());
    };
    match locate(&shown) {
        Found::Here(p) => read_capped(&p, most, whole),
        Found::Arriving => {
            b.arriving = true;
            Err("it is still arriving".into())
        }
        Found::Missing => Err("it isn't on this computer".into()),
    }
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

fn read_model(
    path: &Path,
    venue: &Venue,
    locate: &dyn Fn(&Path) -> Found,
) -> Result<LoadedVenue, Unread> {
    let name = file_name(path);
    let failed = Unread::Failed;
    let local = match locate(path) {
        Found::Here(p) => p,
        Found::Arriving => return Err(Unread::Arriving),
        Found::Missing => return Err(failed(format!("{name} isn't on this computer."))),
    };
    let opened = |e: String| failed(format!("{name} can't be opened: {e}"));
    // A regular file only (a pipe or a device would be read for ever).
    let head = read_capped(&local, 32, false).map_err(opened)?;
    let mut b = Builder::new(venue.brightness());
    let read = match format_of(path, &head) {
        Some(Format::Gltf) => {
            let bytes = read_capped(&local, u64::MAX, false).map_err(opened)?;
            read_gltf(path, &bytes, &mut b, locate)
        }
        Some(Format::Obj) => read_obj(path, &local, &mut b, locate),
        Some(Format::Fbx) => return Err(failed(FBX_WORDS.into())),
        None => return Err(failed(format!("{name}: {FORMAT_WORDS}"))),
    };
    if b.arriving {
        return Err(Unread::Arriving);
    }
    read.map_err(failed)?;
    b.finish(&name, venue).map_err(failed)
}

// ----- Building the parts ------------------------------------------------------

/// A material, as far as the venue view cares.
#[derive(Clone, Debug)]
pub(super) struct Look {
    pub(super) colour: [f32; 4],
    pub(super) texture: Option<Arc<image::RgbaImage>>,
}

pub(super) struct Builder {
    brightness: f32,
    /// Per material key, its look and its triangles so far.
    parts: Vec<(Look, Mesh)>,
    by_key: HashMap<String, usize>,
    triangles: usize,
    left_out: usize,
    warnings: Vec<String>,
    bounds: Option<Bounds>,
    /// A file the model needs is still on its way (`Found::Arriving`).
    arriving: bool,
    /// `MAX_TRIANGLES` and `WARN_TRIANGLES` (smaller in tests).
    limit: usize,
    warn_at: usize,
    /// `MAX_PART_VERTICES` (smaller in tests).
    part_vertices: usize,
}

impl Builder {
    pub(super) fn new(brightness: f32) -> Self {
        Builder {
            brightness,
            parts: Vec::new(),
            by_key: HashMap::new(),
            triangles: 0,
            left_out: 0,
            warnings: Vec::new(),
            bounds: None,
            arriving: false,
            limit: MAX_TRIANGLES,
            warn_at: WARN_TRIANGLES,
            part_vertices: MAX_PART_VERTICES,
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
    pub(super) fn add(
        &mut self,
        key: &str,
        look: impl FnOnce() -> Look,
        mut piece: Mesh,
        m: &Mat4,
    ) {
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

    pub(super) fn finish(mut self, name: &str, venue: &Venue) -> Result<LoadedVenue, String> {
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
        let mut parts = Vec::new();
        for (look, mesh) in self.parts {
            if mesh.indices.is_empty() {
                continue;
            }
            let paint = paint_of(&look, brightness, &mut pictures);
            // In pieces the graphics card takes, with only the corners the
            // triangles kept use.
            for mesh in split_mesh(mesh, self.part_vertices) {
                for p in &mesh.positions {
                    self.bounds = Some(match self.bounds {
                        None => Bounds { min: *p, max: *p },
                        Some(b) => b.with(*p),
                    });
                }
                let mesh_key = mesh.content_key();
                parts.push(VenuePart {
                    mesh: Arc::new(mesh),
                    mesh_key,
                    paint: paint.clone(),
                });
            }
        }
        Ok(LoadedVenue {
            parts,
            bounds: self.bounds,
            triangles: self.triangles,
            warnings: self.warnings,
            loaded_for: venue.clone(),
        })
    }
}

/// `mesh` in pieces of at most `most` corners each, every piece holding only
/// the corners its triangles use (a model cut at `MAX_TRIANGLES` keeps no
/// corners of the triangles left out). The mesh itself when it fits and
/// every corner is used.
fn split_mesh(mesh: Mesh, most: usize) -> Vec<Mesh> {
    let most = most.max(3);
    let n = mesh.positions.len();
    let mut used = vec![false; n];
    for i in &mesh.indices {
        if let Some(u) = used.get_mut(*i as usize) {
            *u = true;
        }
    }
    if n <= most && used.iter().all(|u| *u) {
        return vec![mesh];
    }
    drop(used);
    let mut remap = vec![u32::MAX; n];
    let mut taken: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    let mut cur = Mesh::default();
    for t in mesh.indices.as_chunks::<3>().0 {
        if t.iter().any(|i| *i as usize >= n) {
            continue;
        }
        let new = t
            .iter()
            .enumerate()
            .filter(|(k, i)| remap[**i as usize] == u32::MAX && !t[..*k].contains(i))
            .count();
        if cur.positions.len() + new > most {
            for i in taken.drain(..) {
                remap[i as usize] = u32::MAX;
            }
            out.push(std::mem::take(&mut cur));
        }
        for &i in t {
            let r = &mut remap[i as usize];
            if *r == u32::MAX {
                *r = cur.positions.len() as u32;
                let i = i as usize;
                cur.positions.push(mesh.positions[i]);
                cur.normals
                    .push(mesh.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]));
                cur.uvs.push(mesh.uvs.get(i).copied().unwrap_or_default());
                taken.push(i as u32);
            }
            cur.indices.push(*r);
        }
    }
    if !cur.indices.is_empty() {
        out.push(cur);
    }
    out
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

fn read_gltf(
    path: &Path,
    bytes: &[u8],
    b: &mut Builder,
    locate: &dyn Fn(&Path) -> Found,
) -> Result<(), String> {
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
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in doc.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .take()
                .ok_or_else(|| format!("{name} is missing its binary part."))?,
            // No more than the file says the buffer holds.
            gltf::buffer::Source::Uri(uri) => {
                read_uri(uri, path, buffer.length() as u64, false, locate, b)
                    .map_err(|e| format!("{name}'s data ({}) can't be read: {e}", short(uri)))?
            }
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
                        .or_insert_with(|| gltf_picture(&img, &buffers, path, locate, b))
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
    model: &Path,
    locate: &dyn Fn(&Path) -> Found,
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
        gltf::image::Source::Uri { uri, .. } => {
            match read_uri(uri, model, MAX_PICTURE_FILE, true, locate, b) {
                Ok(d) => d,
                Err(e) => {
                    b.warn(format!(
                        "The picture {} can't be read ({e}); its part is drawn in colour.",
                        short(uri)
                    ));
                    return None;
                }
            }
        }
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
/// model (`read_side`).
fn read_uri(
    uri: &str,
    model: &Path,
    most: u64,
    whole: bool,
    locate: &dyn Fn(&Path) -> Found,
    b: &mut Builder,
) -> Result<Vec<u8>, String> {
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
    read_side(model, &percent_decode(uri), most, whole, locate, b)
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

/// `path` as the show names it (the names of its materials file and
/// pictures are beside it), `local` where it is read from.
fn read_obj(
    path: &Path,
    local: &Path,
    b: &mut Builder,
    locate: &dyn Fn(&Path) -> Found,
) -> Result<(), String> {
    let name = file_name(path);
    let options = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ignore_points: true,
        ignore_lines: true,
    };
    let file = std::fs::File::open(local).map_err(|e| format!("{name} can't be opened: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    // Its materials files, through `read_side` (so never outside its folder,
    // never a pipe, never past `MAX_MATERIALS_FILE`).
    let side = std::cell::RefCell::new((false, Vec::<String>::new()));
    let (models, materials) = tobj::load_obj_buf(&mut reader, &options, |mtl| {
        let mut probe = Builder::new(1.0);
        let text = mtl.to_string_lossy();
        match read_side(path, &text, MAX_MATERIALS_FILE, true, locate, &mut probe) {
            Ok(bytes) => tobj::load_mtl_buf(&mut std::io::Cursor::new(bytes)),
            Err(e) => {
                let mut s = side.borrow_mut();
                s.0 |= probe.arriving;
                s.1.push(format!("{text}: {e}"));
                Err(tobj::LoadError::OpenFileFailed)
            }
        }
    })
    .map_err(|e| format!("{name} can't be read as OBJ: {e}."))?;
    let (arriving, why) = side.into_inner();
    b.arriving |= arriving;
    let materials = match materials {
        Ok(m) => m,
        Err(e) => {
            let e = if why.is_empty() {
                e.to_string()
            } else {
                why.join("; ")
            };
            b.warn(format!(
                "{name}'s materials file can't be read ({e}); the model is drawn in grey."
            ));
            Vec::new()
        }
    };
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
                    let file = match texture_file(path, t, locate) {
                        Found::Here(f) => f,
                        Found::Arriving => {
                            b.arriving = true;
                            return None;
                        }
                        Found::Missing => {
                            b.warn(format!(
                                "The picture {t} isn't beside the model; its part is drawn in \
                                 colour."
                            ));
                            return None;
                        }
                    };
                    pictures
                        .entry(file.clone())
                        .or_insert_with(|| match read_capped(&file, MAX_PICTURE_FILE, true) {
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

/// The names an MTL's `map_Kd` picture may be found by, most likely first:
/// its name may have Windows' `\`, options before it (`-bm 1 wall.png`), or
/// a folder from another computer (only its file name is looked for then).
fn texture_tries(name: &str) -> Vec<String> {
    let name = name.trim();
    let mut tries = vec![name.to_string()];
    if let Some(last) = name.split_whitespace().last() {
        tries.push(last.to_string());
    }
    if let Some(file) = name.rsplit(['/', '\\']).next() {
        tries.push(file.to_string());
    }
    tries.dedup();
    tries
}

/// Where an MTL's `map_Kd` picture is, beside the model at `model`: the
/// first of `texture_tries` that is a file here. One the show network's
/// primary didn't have either is `Missing`; one still coming, `Arriving`.
fn texture_file(model: &Path, name: &str, locate: &dyn Fn(&Path) -> Found) -> Found {
    let mut missing = false;
    let mut arriving = false;
    for t in texture_tries(name) {
        let Some(shown) = side_path(model, &t) else {
            continue;
        };
        match locate(&shown) {
            Found::Here(p) if p.is_file() => return Found::Here(p),
            Found::Here(_) | Found::Missing => missing = true,
            Found::Arriving => arriving = true,
        }
    }
    // (Only one of the names is in a primary's list of files: the others
    // are "on their way" until that list is the show's.)
    if arriving && !missing {
        Found::Arriving
    } else {
        Found::Missing
    }
}

// ----- The files a model reads ----------------------------------------------

/// The most of a glTF's JSON (or an OBJ's materials file) `venue_files` reads.
const MOST_JSON: u64 = 64 << 20;

/// Whether a file is a venue model NewQ reads (by its name).
pub fn is_model_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    matches!(ext.as_deref(), Some("glb" | "gltf" | "obj"))
}

/// The files the venue model at `path` reads besides itself, as
/// `side_path` names them (each once, in the order met): a glTF's buffers
/// and pictures kept in files of their own; an OBJ's materials files and
/// the pictures they name. What a show network's primary hands its
/// computers with the model. A name the model gives that isn't there is
/// listed too (the primary's list says it is missing, so a computer
/// doesn't wait for it). Reads the model (a .glb only as far as its JSON);
/// nothing for a file it can't read.
pub fn venue_files(path: &Path) -> Vec<PathBuf> {
    let Ok(head) = read_capped(path, 32, false) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    match format_of(path, &head) {
        Some(Format::Gltf) => {
            let json = if head.starts_with(b"glTF") {
                // The header, then the first chunk: the JSON.
                let len = head
                    .get(12..16)
                    .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                let is_json = head.get(16..20) == Some(&0x4E4F_534Au32.to_le_bytes()[..]);
                if !is_json {
                    return Vec::new();
                }
                read_capped(path, 20 + u64::from(len).min(MOST_JSON), false)
                    .map(|b| b.get(20..).unwrap_or_default().to_vec())
            } else {
                read_capped(path, MOST_JSON, true)
            };
            let Ok(json) = json else {
                return Vec::new();
            };
            let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&json) else {
                return Vec::new();
            };
            for list in ["buffers", "images"] {
                for item in doc[list].as_array().into_iter().flatten() {
                    if let Some(uri) = item["uri"].as_str()
                        && !uri.starts_with("data:")
                        && let Some(p) = side_path(path, &percent_decode(uri))
                    {
                        add(p);
                    }
                }
            }
        }
        Some(Format::Obj) => {
            let mut materials = Vec::new();
            for_lines(path, u64::MAX, &mut |line| {
                if line.split_whitespace().next() == Some("mtllib")
                    && let Some((_, name)) = line.split_once(' ')
                    && let Some(p) = side_path(path, name)
                {
                    materials.push(p);
                }
            });
            for mtl in materials {
                add(mtl.clone());
                for_lines(&mtl, MOST_JSON, &mut |line| {
                    if line.split_whitespace().next() != Some("map_Kd") {
                        return;
                    }
                    let Some(name) = line.get(6..).map(str::trim).filter(|n| !n.is_empty()) else {
                        return;
                    };
                    // As `texture_file` looks: the first that is there, or
                    // else the first it would have looked for.
                    let tries: Vec<PathBuf> = texture_tries(name)
                        .iter()
                        .filter_map(|t| side_path(path, t))
                        .collect();
                    if let Some(p) = tries.iter().find(|p| p.is_file()).or(tries.first()) {
                        add(p.clone());
                    }
                });
            }
        }
        _ => {}
    }
    out
}

/// Each line of a regular file (at most `most` bytes long), trimmed.
fn for_lines(path: &Path, most: u64, each: &mut dyn FnMut(&str)) {
    use std::io::BufRead as _;
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= most) {
        return;
    }
    let Ok(f) = std::fs::File::open(path) else {
        return;
    };
    let mut r = std::io::BufReader::new(f);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => each(String::from_utf8_lossy(&buf).trim()),
        }
    }
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
