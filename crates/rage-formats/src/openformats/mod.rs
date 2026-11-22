//! OpenIV's openFormats: GTA V models and textures as text files.
//!
//! OpenIV exports drawables (`.ydr` → `.odr`), fragments (`.yft` → `.oft`),
//! drawable dictionaries (`.ydd` → `.odd`) and texture dictionaries
//! (`.ytd` → `.otd`) as plain text, with the meshes, skeletons and textures
//! in a folder named after the file. The layout here follows OpenIV's own
//! exports (props and their `.mesh`, `.skel`, `.otd`/`.otx` files):
//!
//! * every file starts with `Version <major> <minor>`, then a block in
//!   braces; a line `Key values…` is a value, a line followed by `{ … }` a
//!   block; paths are relative to the file and use backslashes;
//! * `.odr`: `Shaders` (one `<name>.sps` block per shader with its
//!   parameters: texture names and numbers), `Skeleton <path>.skel` or
//!   `null`, `LodGroup` (`High`/`Med`/`Low`/`Vlow` with their distance and
//!   the `.mesh` files and the bone each is attached to, `Center`, `Radius`,
//!   `AABBMin`, `AABBMax`), `Joints`, `Light`, `Bound`;
//! * `.mesh`: `Locked`, `Skinned`, `BoneCount`, `Mask`, `Bounds` (one
//!   `Aabb` around everything when there are several geometries, then one
//!   per geometry), `Geometries` with each `Geometry`'s `ShaderIndex`,
//!   `Flags`, `VertexDeclaration`, `Indices <n> { … }` (15 per line) and
//!   `Vertices <n> { … }`: one vertex per line, its components separated by
//!   ` / ` in declaration order (position, blend weights, blend indices,
//!   normal, colours, texture coordinates, tangent, binormal);
//! * `.skel`: `DataCRC`, `NumBones`, then the bone tree: `Bone <name> <tag>`
//!   with `MirrorBoneId`, `Flags` (`ROT_X` … `TRANS_Z`), `RotationQuaternion`,
//!   `LocalOffset`, `Scale` and `Children <n> { … }`;
//! * `.otd`: the `.otx` file of every texture; `.otx`: `Image <file>.dds`,
//!   `Type`, `PixelFormat`, `Levels`, `Usage`, `UsageFlags`, `ExtraFlags`;
//! * `.odd`: the `.odr` file of every drawable.
//!
//! Vertex declarations are named by OpenIV with `N` (fixed) or `S`
//! (skinned) and a hash of the layout: [`VertexDecl::code`] knows the codes
//! seen in OpenIV's exports and writes other layouts as `N`/`S`, the
//! component flags and the 4-bit component types in hex (e.g.
//! `N40597755555555996996`), which OpenRAGE reads back but OpenIV may not.
//!
//! Not from OpenIV's exports, so modelled on the rest: the embedded texture
//! dictionary of a drawable (`TextureDictionary <path>.otd` after the
//! shaders), and `.oft` files (the main drawable as a `Drawable` block with
//! the `.odr` contents, then a `Children` block with each part's `BoneTag`
//! and `Drawable`); the fragment's physics are not exported. Shader
//! parameters of GTA V Enhanced models are their textures only (their
//! numbers live in constant buffers that are not read yet).

mod compare;
mod parse;
mod read;
mod text;
mod write;

pub use compare::{difference, texture_difference};
pub use parse::{Source, parse_odd, parse_odr, parse_oft, parse_otd};
pub use read::{read_drawable_at, read_ydd, read_ydr, read_yft, read_ytd};
pub use write::{ExportedFile, export_odd, export_odr, export_oft, export_otd, file_stem, same_texture_name};

use crate::texture::Texture;

/// Resource versions OpenIV writes into the text files' `Version` lines.
pub const VERSION_DRAWABLE: (u32, u32) = (165, 32);
pub const VERSION_SKELETON: (u32, u32) = (165, 30);
pub const VERSION_FRAGMENT: (u32, u32) = (162, 30);
pub const VERSION_TEXTURES: (u32, u32) = (13, 30);

/// A drawable: shaders, skeleton, up to four detail levels.
#[derive(Debug, Clone, Default)]
pub struct OfDrawable {
    /// The name the files are written under (without extension).
    pub name: String,
    pub shaders: Vec<OfShader>,
    pub skeleton: Option<OfSkeleton>,
    /// High, medium, low and very low detail.
    pub lods: [OfLod; 4],
    pub center: [f32; 3],
    pub radius: f32,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// Textures embedded in the drawable.
    pub textures: Vec<OfTexture>,
}

impl OfDrawable {
    /// Every geometry of every model of every level.
    pub fn geometries(&self) -> impl Iterator<Item = &OfGeometry> {
        self.lods.iter().flat_map(|l| &l.models).flat_map(|m| &m.geometries)
    }
}

/// The names of the detail levels in `.odr` files.
pub const LOD_NAMES: [&str; 4] = ["High", "Med", "Low", "Vlow"];

#[derive(Debug, Clone)]
pub struct OfLod {
    /// How far away the level is drawn (9998 when not set).
    pub distance: f32,
    pub models: Vec<OfModel>,
}

impl Default for OfLod {
    fn default() -> Self {
        Self { distance: 9998.0, models: Vec::new() }
    }
}

/// A model: geometries drawn together, attached to one bone (or skinned).
#[derive(Debug, Clone, Default)]
pub struct OfModel {
    /// The bone the model moves with (fixed models).
    pub bone: u8,
    pub skinned: bool,
    /// Bones the model's vertices are skinned to.
    pub bone_count: u8,
    /// Render mask (`0xFF`: always).
    pub mask: u8,
    /// One box around all geometries when there are several, then one per geometry.
    pub bounds: Vec<([f32; 3], [f32; 3])>,
    pub geometries: Vec<OfGeometry>,
}

/// Vertex components in declaration order, as GTA V Legacy numbers them.
pub mod component {
    pub const POSITION: usize = 0;
    pub const BLEND_WEIGHTS: usize = 1;
    pub const BLEND_INDICES: usize = 2;
    pub const NORMAL: usize = 3;
    pub const COLOUR0: usize = 4;
    pub const COLOUR1: usize = 5;
    /// Texture coordinates 0–7 are components 6–13.
    pub const TEXCOORD0: usize = 6;
    pub const TANGENT: usize = 14;
    pub const BINORMAL: usize = 15;
}

/// The 4-bit component types of GTA V Legacy's vertex declarations, which
/// nearly every model uses: float3 positions and normals, bytes for blend
/// weights, indices and colours, float2 texture coordinates, float4 tangents.
pub const DEFAULT_TYPES: u64 = 0x7755_5555_5599_6996;

/// A vertex declaration: which components the vertices have
/// (`flags`, bit per [`component`]) and how they are stored (`types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexDecl {
    pub flags: u32,
    pub types: u64,
}

/// Declaration codes seen in OpenIV's exports (all with [`DEFAULT_TYPES`]).
const KNOWN_DECLS: &[(&str, u32)] = &[
    ("N209731BE", 0x0059), // position, normal, colour, uv
    ("N51263BB5", 0x4059), // … tangent
    ("S9445853F", 0x405F), // skinned, position, normal, colour, uv, tangent
    ("SBED48839", 0x007F), // skinned, two colours, uv (ped_default)
    ("SD7D22350", 0x407F), // … tangent (ped_hair_cutout_alpha)
    ("S12D0183F", 0x40FF), // … two uv sets (ped)
];

impl VertexDecl {
    /// The components present, in order, with their 4-bit type.
    pub fn components(self) -> impl Iterator<Item = (usize, u8)> {
        (0..16).filter(move |k| self.flags >> k & 1 != 0).map(move |k| (k, (self.types >> (k * 4) & 0xF) as u8))
    }

    pub fn has(self, component: usize) -> bool {
        self.flags >> component & 1 != 0
    }

    pub fn skinned(self) -> bool {
        self.has(component::BLEND_WEIGHTS)
    }

    /// The types bits of the components present (others are irrelevant).
    fn used_types(self) -> u64 {
        self.components().fold(0, |acc, (k, t)| acc | (t as u64) << (k * 4))
    }

    /// The same components stored the same way (unused type bits aside).
    pub fn same_layout(self, other: Self) -> bool {
        self.flags == other.flags && self.used_types() == other.used_types()
    }

    /// OpenIV's name of the declaration.
    pub fn code(self) -> String {
        let used = self.used_types();
        let default = self.components().fold(0u64, |acc, (k, _)| acc | (DEFAULT_TYPES >> (k * 4) & 0xF) << (k * 4));
        if used == default
            && let Some((code, _)) = KNOWN_DECLS.iter().find(|(_, f)| *f == self.flags)
        {
            return (*code).to_owned();
        }
        format!("{}{:04X}{:016X}", if self.skinned() { 'S' } else { 'N' }, self.flags, self.types)
    }

    /// The declaration a code names ([`VertexDecl::code`]).
    pub fn from_code(code: &str) -> Option<Self> {
        if let Some((_, flags)) = KNOWN_DECLS.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)) {
            return Some(Self { flags: *flags, types: DEFAULT_TYPES });
        }
        let hex = code.get(1..).filter(|h| h.len() == 20 && code.starts_with(['N', 'S', 'n', 's']))?;
        Some(Self { flags: u32::from_str_radix(&hex[..4], 16).ok()?, types: u64::from_str_radix(&hex[4..], 16).ok()? })
    }

    /// How many numbers a component has in the text.
    pub fn width(component: usize, ty: u8) -> usize {
        match (component, ty) {
            (component::BLEND_WEIGHTS | component::BLEND_INDICES | component::COLOUR0 | component::COLOUR1, _) => 4,
            (_, 2) => 1,
            (_, 1 | 5) => 2,
            (_, 6) => 3,
            _ => 4,
        }
    }

    /// Whether a component is written as whole numbers (bytes).
    pub fn integral(component: usize) -> bool {
        matches!(component, component::BLEND_INDICES | component::COLOUR0 | component::COLOUR1)
    }
}

/// A geometry: triangles of one shader.
#[derive(Debug, Clone)]
pub struct OfGeometry {
    pub shader: u16,
    pub decl: VertexDecl,
    pub indices: Vec<u32>,
    /// One list per component of the declaration, in its order; each with a
    /// value per vertex. Blend indices are skeleton bone indices, colours and
    /// blend indices 0–255, blend weights 0–1.
    pub attributes: Vec<(usize, Vec<[f32; 4]>)>,
}

impl OfGeometry {
    pub fn vertex_count(&self) -> usize {
        self.attributes.first().map_or(0, |(_, v)| v.len())
    }

    /// The values of a component, if the geometry has it.
    pub fn attribute(&self, component: usize) -> Option<&[[f32; 4]]> {
        self.attributes.iter().find(|(c, _)| *c == component).map(|(_, v)| v.as_slice())
    }
}

/// A shader and its parameters.
#[derive(Debug, Clone, Default)]
pub struct OfShader {
    /// Its name without `.sps`, e.g. `normal_spec`.
    pub name: String,
    pub params: Vec<(String, OfValue)>,
}

impl OfShader {
    /// The texture of the parameter `name` (case-insensitive).
    pub fn texture(&self, name: &str) -> Option<&str> {
        self.params.iter().find_map(|(n, v)| match v {
            OfValue::Texture(t) if n.eq_ignore_ascii_case(name) => Some(t.as_str()),
            _ => None,
        })
    }

    /// All textures, in order.
    pub fn textures(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter().filter_map(|(n, v)| match v {
            OfValue::Texture(t) => Some((n.as_str(), t.as_str())),
            OfValue::Floats(_) => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OfValue {
    Texture(String),
    Floats(Vec<f32>),
}

#[derive(Debug, Clone, Default)]
pub struct OfSkeleton {
    pub crc: u32,
    pub bones: Vec<OfBone>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OfBone {
    pub name: String,
    pub tag: u16,
    pub mirror: u16,
    /// Index of the parent, -1 for the root.
    pub parent: i16,
    /// `ROT_X` … (see [`BONE_FLAGS`]).
    pub flags: u16,
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

/// Bone flag names, by bit.
pub const BONE_FLAGS: &[(u16, &str)] = &[
    (0x1, "ROT_X"),
    (0x2, "ROT_Y"),
    (0x4, "ROT_Z"),
    (0x8, "LIMIT_ROTATION"),
    (0x10, "TRANS_X"),
    (0x20, "TRANS_Y"),
    (0x40, "TRANS_Z"),
    (0x80, "LIMIT_TRANSLATION"),
    (0x100, "SCALE_X"),
    (0x200, "SCALE_Y"),
    (0x400, "SCALE_Z"),
    (0x800, "LIMIT_SCALE"),
];

/// A texture with what the `.otx` file says about it.
#[derive(Debug, Clone)]
pub struct OfTexture {
    pub texture: Texture,
}

/// A fragment: its main drawable and its parts (wheels, …) on bones.
#[derive(Debug, Clone, Default)]
pub struct OfFragment {
    pub drawable: OfDrawable,
    /// (bone tag, drawable)
    pub children: Vec<(u16, OfDrawable)>,
}

/// Texture usage names in `.otx` files, by the game's usage code.
pub(crate) const USAGES: &[(u8, &str)] = &[
    (1, "DEFAULT"),
    (2, "TERRAIN"),
    (3, "CLOUDDENSITY"),
    (4, "CLOUDNORMAL"),
    (5, "CABLE"),
    (6, "FENCE"),
    (8, "SCRIPT"),
    (9, "WATERFLOW"),
    (10, "WATERFOAM"),
    (11, "WATERFOG"),
    (12, "WATEROCEAN"),
    (14, "FOAMOPACITY"),
    (16, "DIFFUSEMIPSHARPEN"),
    (18, "DIFFUSEDARK"),
    (19, "DIFFUSEALPHAOPAQUE"),
    (20, "DIFFUSE"),
    (21, "DETAIL"),
    (22, "NORMAL"),
    (23, "SPECULAR"),
    (24, "EMISSIVE"),
    (25, "TINTPALETTE"),
    (26, "SKIPPROCESSING"),
];

/// Shader parameter names (the game stores their hashes) and how many
/// numbers each has; textures have 0.
pub(crate) const PARAMS: &[(&str, usize)] = &[
    ("DiffuseSampler", 0),
    ("DiffuseSampler2", 0),
    ("DiffuseSampler3", 0),
    ("BumpSampler", 0),
    ("BumpSampler2", 0),
    ("SpecSampler", 0),
    ("DetailSampler", 0),
    ("DetailSampler2", 0),
    ("EnvironmentSampler", 0),
    ("DirtSampler", 0),
    ("DamageSampler", 0),
    ("DamageMaskSampler", 0),
    ("DiffuseExtraSampler", 0),
    ("DiffuseHfSampler", 0),
    ("PlateBgSampler", 0),
    ("PlateBgBumpSampler", 0),
    ("FontSampler", 0),
    ("FontNormalSampler", 0),
    ("TintPaletteSampler", 0),
    ("PaletteSampler", 0),
    ("DistanceMapSampler", 0),
    ("HeightSampler", 0),
    ("LookupSampler", 0),
    ("SnowSampler", 0),
    ("StubbleSampler", 0),
    ("WrinkleSampler_A", 0),
    ("WrinkleSampler_B", 0),
    ("WrinkleMaskSampler_0", 0),
    ("WrinkleMaskSampler_1", 0),
    ("WrinkleMaskSampler_2", 0),
    ("WrinkleMaskSampler_3", 0),
    ("WrinkleMaskSampler_4", 0),
    ("WrinkleMaskSampler_5", 0),
    ("TextureSampler_layer0", 0),
    ("TextureSampler_layer1", 0),
    ("TextureSampler_layer2", 0),
    ("TextureSampler_layer3", 0),
    ("BumpSampler_layer0", 0),
    ("BumpSampler_layer1", 0),
    ("BumpSampler_layer2", 0),
    ("BumpSampler_layer3", 0),
    ("HeightMapSamplerLayer0", 0),
    ("HeightMapSamplerLayer1", 0),
    ("HeightMapSamplerLayer2", 0),
    ("HeightMapSamplerLayer3", 0),
    ("ComboHeightSamplerFur", 0),
    ("ComboHeightSamplerFur2", 0),
    ("ComboHeightSamplerFur3", 0),
    ("ComboHeightSamplerFur4", 0),
    ("NoiseSampler", 0),
    ("FlowSampler", 0),
    ("FoamSampler", 0),
    ("AnisoNoiseSpecSampler", 0),
    ("StippleSampler", 0),
    ("MaskSampler", 0),
    ("DiffuseTex", 0),
    ("BumpTex", 0),
    ("SpecularTex", 0),
    ("DetailTex", 0),
    ("DirtTex", 0),
    ("DamageTexture", 0),
    ("DiffuseTex2", 0),
    ("EnvironmentTex", 0),
    ("Bumpiness", 1),
    ("SpecularIntensityMult", 1),
    ("SpecularFalloffMult", 1),
    ("SpecularFresnel", 1),
    ("SpecMapIntMask", 3),
    ("ReflectivePower", 1),
    ("DetailSettings", 4),
    ("UseTessellation", 1),
    ("HardAlphaBlend", 1),
    ("WetnessMultiplier", 1),
    ("EmissiveMultiplier", 1),
    ("AlphaScale", 1),
    ("AlphaTestValue", 1),
    ("matDiffuseColor", 4),
    ("matDiffuseColor2", 4),
    ("DirtLevelMod", 1),
    ("DirtColor", 3),
    ("DirtDecalMask", 4),
    ("EnvEffScale", 2),
    ("EnvEffThickness", 1),
    ("EnvEffTexTileUV", 1),
    ("Specular2ColorIntensity", 1),
    ("Specular2Factor", 1),
    ("Specular2Color", 3),
    ("Specular2Color_DirLerp", 4),
    ("FresnelRolloff", 1),
    ("ParallaxScaleBias", 1),
    ("POMDisable", 1),
    ("globalAnimUV0", 3),
    ("globalAnimUV1", 3),
    ("TintPaletteSelector", 2),
    ("WindGlobalParams", 4),
    ("umGlobalParams", 3),
    ("umGlobalOverrideParams", 1),
    ("ZShift", 1),
    ("LetterIndex1", 4),
    ("LetterIndex2", 4),
    ("LetterSize", 2),
    ("NumLetters", 1),
    ("LicensePlateFontExtents", 4),
    ("LicensePlateFontTint", 4),
    ("FontNormalScale", 1),
    ("DistEpsilonScaleMin", 4),
    ("DistMapCenterVal", 4),
    ("FillColor", 4),
    ("StubbleControl", 4),
    ("StubbleGrowth", 1),
    ("DiffuseMod", 4),
    ("NormalMapMult", 1),
    ("DetailPower", 1),
    ("DetailScale", 1),
    ("DetailBumpiness", 1),
    ("DetailOffset", 4),
    ("DetailMapScale", 1),
    ("NoiseSettings", 4),
    ("RippleSpeed", 1),
    ("RippleScale", 1),
    ("RippleBumpiness", 1),
    ("WaveOffset", 1),
    ("WaveMovement", 1),
    ("WaveSpeed", 1),
    ("FogColor", 3),
    ("SpecularColor", 3),
    ("GlassColor", 4),
    ("Selfillumcolor", 3),
    ("ClothParameters", 4),
    ("OrderNumber", 1),
    ("UVScrollSpeed", 2),
    ("ScanlineFilterParams", 4),
    ("TextureAnimationParams", 4),
    ("MaterialWetnessMultiplier", 1),
    ("BrokenGlassTileScale", 1),
    ("BrokenDiffuseColor", 4),
    ("BrokenSpecularColor", 4),
    ("CrackLevelMin", 1),
    ("CrackLevelMax", 1),
    ("CrackLevel_reciprocal", 1),
    ("CrackEdgeBumpTileScale", 1),
    ("CrackEdgeBumpAmount", 1),
    ("CrackDecalBumpTileScale", 1),
    ("CrackDecalBumpAmount", 1),
    ("CrackDecalBumpAlphaThreshold", 1),
    ("DecalTint", 4),
    ("DamagedWheelOffsets", 4),
    ("EnvironmentMapIntensity", 1),
    ("WireframeColor", 4),
];

/// The name of a shader parameter hash: known names, else `hash_XXXXXXXX`.
pub(crate) fn param_name(hash: u32) -> (String, Option<usize>) {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static TABLE: OnceLock<HashMap<u32, (&'static str, usize)>> = OnceLock::new();
    let table = TABLE.get_or_init(|| PARAMS.iter().map(|&(n, w)| (rage_core::hash::joaat(n), (n, w))).collect());
    match table.get(&hash) {
        Some(&(n, w)) => (n.to_owned(), Some(w)),
        None => (format!("hash_{hash:08X}"), None),
    }
}

/// The hash of a parameter name as [`param_name`] writes it.
pub fn param_hash(name: &str) -> u32 {
    match name.strip_prefix("hash_").and_then(|h| u32::from_str_radix(h, 16).ok()) {
        Some(h) => h,
        None => rage_core::hash::joaat(name),
    }
}

/// The hash of a shader name as written (`<name>.sps` without the extension).
pub fn shader_hash(name: &str) -> u32 {
    param_hash(name)
}

/// The name a shader hash is written as.
pub(crate) fn shader_name(hash: u32) -> String {
    crate::shaders::name(hash).map(str::to_owned).unwrap_or_else(|| format!("hash_{hash:08X}"))
}

#[cfg(test)]
mod tests;
