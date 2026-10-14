//! Import-time LODs (renderer pass 2, A6.9): the `genex_lods` extras the Studio's import step
//! writes on a GLB primitive (`src/substrate/asset-import/lods.ts`), read and validated here, merged
//! level by level across a part's primitives, and the level a draw takes ([`select_lod`], and its
//! WGSL twin [`LOD_SELECT_WGSL`]).
//!
//! A primitive's levels are triangle lists over its own vertices, coarser and coarser, each with
//! the object-space error the simplifier reported. A part (one material's primitives merged into
//! one mesh) gets a chain only when every one of its primitives brought valid levels: level k is
//! every primitive's level k rebased into the part, its error the largest of theirs scaled by
//! each primitive's node. A set that fails is dropped with a typed [`LodFault`] and the mesh
//! draws whole. The cull choosing a level per instance and drawing its index range is the
//! renderer's follow-up; here the chain rides along on [`crate::mesh::GlbPart`].

use glam::{Mat3, Mat4};

/// The key of the LOD object in a glTF primitive's `extras` (never an extension).
pub const LODS_EXTRAS_KEY: &str = "genex_lods";
/// The `genex_lods` schema version this build reads; any other is ignored with a typed fault.
pub const LOD_SCHEMA_VERSION: u32 = 1;
/// Most levels past the primitive's own triangles (the WGSL select reads them as a `vec4`).
pub const LOD_MAX_LEVELS: u32 = 4;
/// The fewest triangles a GLB needs before the import step simplifies it.
pub const LOD_MIN_TRIANGLES: u32 = 512;
/// [`select_lod`] in WGSL (`gx_lod_select`), for the cull to include.
pub const LOD_SELECT_WGSL: &str = include_str!("../shaders/geometry/lod_select.wgsl");

/// The members of the `genex_lods` extras object (wire spellings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodExtrasKey {
    /// The schema version ([`LOD_SCHEMA_VERSION`]).
    Version,
    /// The bufferView of every level's triangle list: little-endian `u32` primitive vertex indices.
    Indices,
    /// The levels, coarser and coarser ([`LodLevelKey`] each).
    Levels,
}

impl LodExtrasKey {
    /// Every key, in the schema's order.
    pub const ALL: [LodExtrasKey; 3] = [LodExtrasKey::Version, LodExtrasKey::Indices, LodExtrasKey::Levels];

    /// The wire spelling.
    pub const fn wire(self) -> &'static str {
        match self {
            LodExtrasKey::Version => "version",
            LodExtrasKey::Indices => "indices",
            LodExtrasKey::Levels => "levels",
        }
    }
}

/// The members of one level (wire spellings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodLevelKey {
    /// Its first index in the view.
    First,
    /// Its index count (whole triangles).
    Count,
    /// Its simplification error, an object-space distance.
    Error,
}

impl LodLevelKey {
    /// Every key, in the schema's order.
    pub const ALL: [LodLevelKey; 3] = [LodLevelKey::First, LodLevelKey::Count, LodLevelKey::Error];

    /// The wire spelling.
    pub const fn wire(self) -> &'static str {
        match self {
            LodLevelKey::First => "first",
            LodLevelKey::Count => "count",
            LodLevelKey::Error => "error",
        }
    }
}

/// Why a primitive's `genex_lods` is refused (typed; the mesh then draws whole).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodFault {
    /// Another schema version, or extras that are not an object.
    UnknownVersion,
    /// More than [`LOD_MAX_LEVELS`] levels.
    OverLimit,
    /// A view that is missing, outside the buffer or not whole words, or a level past its view.
    RangeOutOfBounds,
    /// An index past the primitive's vertices.
    VertexOutOfMesh,
    /// A level that is not whole triangles, finer than the one before, or with an error that is
    /// not finite, negative or smaller than the one before.
    NotCoarser,
}

impl LodFault {
    /// Every fault, in the schema's order.
    pub const ALL: [LodFault; 5] = [LodFault::UnknownVersion, LodFault::OverLimit, LodFault::RangeOutOfBounds, LodFault::VertexOutOfMesh, LodFault::NotCoarser];

    /// The wire spelling (the typed warning).
    pub const fn wire(self) -> &'static str {
        match self {
            LodFault::UnknownVersion => "lods-unknown-version",
            LodFault::OverLimit => "lods-over-limit",
            LodFault::RangeOutOfBounds => "lods-range-out-of-bounds",
            LodFault::VertexOutOfMesh => "lods-vertex-out-of-mesh",
            LodFault::NotCoarser => "lods-not-coarser",
        }
    }
}

/// A primitive whose `genex_lods` were refused: the part then has no chain. `fault.wire()` is the
/// typed warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LodWarning {
    /// The primitive's place among the part's primitives.
    pub primitive: usize,
    /// Why the extras were refused.
    pub fault: LodFault,
}

/// One simplified level of a part.
#[derive(Clone, Debug, PartialEq)]
pub struct LodLevel {
    /// Its triangle list over the part's vertices.
    pub indices: Vec<u32>,
    /// Its object-space error in the part's (placed) space.
    pub error: f32,
}

/// A part's levels, coarser and coarser (level 0, the mesh itself, is not among them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LodChain {
    /// The levels.
    pub levels: Vec<LodLevel>,
}

impl LodChain {
    /// The levels' errors, for [`select_lod`].
    pub fn errors(&self) -> Vec<f32> {
        self.levels.iter().map(|level| level.error).collect()
    }
}

/// The level to draw: 0 for the mesh itself, `k` for `errors[k - 1]`'s level, the coarsest whose
/// error seen at `distance` (with `pixels_per_unit` pixels a world unit at distance 1: the viewport
/// height over twice the tangent of half the vertical field of view) is at most `threshold_px`.
/// Compared as `error * pixels_per_unit <= threshold_px * distance`, two correctly rounded products,
/// exactly as `gx_lod_select` does; a distance that is not positive (or NaN) draws the mesh itself.
pub fn select_lod(errors: &[f32], distance: f32, pixels_per_unit: f32, threshold_px: f32) -> u32 {
    if distance.is_nan() || distance <= 0.0 {
        return 0;
    }
    let budget = threshold_px * distance;
    let mut picked = 0;
    for (level, error) in errors.iter().take(LOD_MAX_LEVELS as usize).enumerate() {
        if error * pixels_per_unit <= budget {
            picked = level as u32 + 1;
        }
    }
    picked
}

/// What one primitive's `extras.genex_lods` gave.
pub(crate) enum PrimitiveLods {
    /// No such member.
    Absent,
    /// Levels that passed validation, over the primitive's own vertices.
    Valid(Vec<LodLevel>),
    /// Levels refused.
    Refused(LodFault),
}

/// A level entry's numbers: first, count, error.
fn level_entry(value: &gltf::json::Value) -> Option<(usize, usize, f32)> {
    let first = usize::try_from(value.get(LodLevelKey::First.wire())?.as_u64()?).ok()?;
    let count = usize::try_from(value.get(LodLevelKey::Count.wire())?.as_u64()?).ok()?;
    let error = value.get(LodLevelKey::Error.wire())?.as_f64()? as f32;
    Some((first, count, error))
}

/// The indices view's words.
fn view_words(gltf: &gltf::Gltf, blob: Option<&[u8]>, extras: &gltf::json::Value) -> Result<Vec<u32>, LodFault> {
    let at = extras.get(LodExtrasKey::Indices.wire()).and_then(gltf::json::Value::as_u64).ok_or(LodFault::RangeOutOfBounds)?;
    let view = usize::try_from(at).ok().and_then(|at| gltf.views().nth(at)).ok_or(LodFault::RangeOutOfBounds)?;
    let range = view.offset()..view.offset().saturating_add(view.length());
    let bytes = blob.and_then(|blob| blob.get(range)).ok_or(LodFault::RangeOutOfBounds)?;
    if !bytes.len().is_multiple_of(4) {
        return Err(LodFault::RangeOutOfBounds);
    }
    Ok(bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
}

/// One level checked against the one before it and the primitive's vertex count.
fn check_level(level: &LodLevel, previous: Option<&LodLevel>, vertex_count: u32) -> Result<(), LodFault> {
    let whole = !level.indices.is_empty() && level.indices.len().is_multiple_of(3);
    let error_ok = level.error.is_finite() && level.error >= previous.map_or(0.0, |p| p.error);
    let coarser = previous.is_none_or(|p| level.indices.len() <= p.indices.len());
    if !(whole && error_ok && coarser) {
        return Err(LodFault::NotCoarser);
    }
    if level.indices.iter().any(|&index| index >= vertex_count) {
        return Err(LodFault::VertexOutOfMesh);
    }
    Ok(())
}

/// The levels an extras member names, every number checked.
fn decode(gltf: &gltf::Gltf, blob: Option<&[u8]>, extras: &gltf::json::Value, vertex_count: u32) -> Result<Vec<LodLevel>, LodFault> {
    let version = extras.get(LodExtrasKey::Version.wire()).and_then(gltf::json::Value::as_u64);
    if !extras.is_object() || version != Some(u64::from(LOD_SCHEMA_VERSION)) {
        return Err(LodFault::UnknownVersion);
    }
    let entries = extras.get(LodExtrasKey::Levels.wire()).and_then(gltf::json::Value::as_array).ok_or(LodFault::RangeOutOfBounds)?;
    if entries.len() > LOD_MAX_LEVELS as usize {
        return Err(LodFault::OverLimit);
    }
    let words = view_words(gltf, blob, extras)?;
    let mut levels: Vec<LodLevel> = Vec::with_capacity(entries.len());
    for entry in entries {
        let (first, count, error) = level_entry(entry).ok_or(LodFault::RangeOutOfBounds)?;
        let indices = first.checked_add(count).and_then(|end| words.get(first..end)).ok_or(LodFault::RangeOutOfBounds)?;
        let level = LodLevel { indices: indices.to_vec(), error };
        check_level(&level, levels.last(), vertex_count)?;
        levels.push(level);
    }
    Ok(levels)
}

/// Read and validate a primitive's `genex_lods` against its `vertex_count` vertices. Extras that
/// are not an object, or have no such member, are not ours ([`PrimitiveLods::Absent`]).
pub(crate) fn read_lod_extras(gltf: &gltf::Gltf, primitive: &gltf::Primitive<'_>, blob: Option<&[u8]>, vertex_count: u32) -> PrimitiveLods {
    let Some(raw) = primitive.extras() else { return PrimitiveLods::Absent };
    let Ok(value) = gltf::json::deserialize::from_str::<gltf::json::Value>(raw.get()) else { return PrimitiveLods::Absent };
    let Some(extras) = value.get(LODS_EXTRAS_KEY) else { return PrimitiveLods::Absent };
    match decode(gltf, blob, extras, vertex_count) {
        Ok(levels) => PrimitiveLods::Valid(levels),
        Err(fault) => PrimitiveLods::Refused(fault),
    }
}

/// How much a node's matrix can stretch a distance: its linear part's longest axis.
fn largest_scale(world: Mat4) -> f32 {
    let linear = Mat3::from_mat4(world);
    [linear.x_axis.length(), linear.y_axis.length(), linear.z_axis.length()].into_iter().fold(0.0_f32, f32::max)
}

/// A part's primitives' levels gathered into its chain.
#[derive(Default)]
pub(crate) struct LodMerge {
    levels: Vec<LodLevel>,
    /// Every primitive visited so far had valid levels.
    complete: bool,
    started: bool,
    visited: usize,
    warnings: Vec<LodWarning>,
}

impl LodMerge {
    /// One primitive's levels, placed by `world`, its vertices starting at `base` in the part.
    pub(crate) fn visit(&mut self, found: PrimitiveLods, world: Mat4, base: u32) {
        if !self.started {
            (self.started, self.complete) = (true, true);
        }
        match found {
            PrimitiveLods::Absent => self.complete = false,
            PrimitiveLods::Refused(fault) => {
                self.complete = false;
                log::warn!("glb lods: primitive {} refused ({}); the mesh draws whole", self.visited, fault.wire());
                self.warnings.push(LodWarning { primitive: self.visited, fault });
            }
            PrimitiveLods::Valid(levels) => self.append(levels, largest_scale(world), base),
        }
        self.visited += 1;
    }

    /// Level k of the part gains the primitive's level k; the part keeps as many levels as its
    /// shortest primitive chain.
    fn append(&mut self, levels: Vec<LodLevel>, scale: f32, base: u32) {
        let first = self.visited == 0;
        if !first {
            self.levels.truncate(levels.len());
        }
        for (at, level) in levels.into_iter().enumerate() {
            let rebased = level.indices.iter().map(|index| base + index);
            let error = level.error * scale;
            match self.levels.get_mut(at) {
                Some(merged) => {
                    merged.indices.extend(rebased);
                    merged.error = merged.error.max(error);
                }
                None if first => self.levels.push(LodLevel { indices: rebased.collect(), error }),
                None => {}
            }
        }
    }

    /// The part's chain when every primitive had levels, with the warnings of those refused.
    pub(crate) fn finish(self) -> (Option<LodChain>, Vec<LodWarning>) {
        let usable = self.complete && !self.levels.is_empty();
        (usable.then_some(LodChain { levels: self.levels }), self.warnings)
    }
}
