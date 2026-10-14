//! Compressed material textures (renderer pass 2, A6.4): a KTX2 file in one fixed GPU format
//! (BC7, ASTC 4x4 or ETC2 RGBA8, each sRGB or linear, or plain RGBA8) is read and checked with the
//! `ktx2` crate, and its stored mip chain goes into size-class arrays of that family as it is, at
//! a quarter of RGBA8's bytes. The compression features are renderer-owned: requested whenever
//! the adapter offers them.
//!
//! The CPU rows run everywhere. The GPU rows are `#[ignore]`d and every name contains `metal`:
//!
//!   GENEX_REQUIRE_METAL=1 node scripts/toolchains/rust-env.ts -- cargo test -p genex-render \
//!     --offline --locked --test texture_compressed -- --ignored metal

use genex_render::budget::MaterialClass;
use genex_render::device_class::DeviceClass;
use genex_render::floor::RENDERER_FEATURES;
use genex_render::settings::{QualityPreset, TextureClass};
use genex_render::texture::{
    ClassArrays, CompressedTexture, TextureData, TextureFamily, TextureKind, material_arrays, mip_level_count, preset_textures, read_ktx2,
};
use genex_render::{Gpu, RenderCode};
use ktx2::{Format, SupercompressionScheme};
use wgpu::Features;

/// What a test KTX2 file holds: its header fields and its levels as stored.
struct Ktx2 {
    format: Option<Format>,
    width: u32,
    height: u32,
    depth: u32,
    layers: u32,
    faces: u32,
    supercompression: u32,
    levels: Vec<Vec<u8>>,
}

impl Ktx2 {
    /// A square texture of `side` texels with its whole chain, block `b` of every level filled
    /// with `seed + level` so each level and block is told apart.
    fn chain(format: Format, side: u32, seed: u8) -> Ktx2 {
        let family = family_of(format);
        let levels = (0..mip_level_count(side))
            .map(|level| {
                let px = (side >> level).max(1);
                let bytes = family.level_bytes(px, px) as usize;
                (0..bytes).map(|at| seed.wrapping_add(level as u8).wrapping_add((at / 16) as u8)).collect()
            })
            .collect();
        Ktx2 { format: Some(format), width: side, height: side, depth: 0, layers: 0, faces: 1, supercompression: 0, levels }
    }

    /// The file: header, level index, a basic DFD for the format and the levels, smallest last.
    fn bytes(&self) -> Vec<u8> {
        let dfd = match self.format {
            Some(format) => ktx2::dfd::Block::Basic(ktx2::dfd::Basic::from_format(format).expect("a DFD for the format").0).to_vec(),
            None => ktx2::dfd::Block::Basic(ktx2::dfd::Basic::from_format(Format::R8G8B8A8_UNORM).expect("dfd").0).to_vec(),
        };
        let index_end = 80 + 24 * self.levels.len();
        let dfd_at = index_end;
        let dfd_len = 4 + dfd.len();
        let mut at = (dfd_at + dfd_len).next_multiple_of(16);
        let mut index = Vec::new();
        for level in &self.levels {
            index.push((at as u64, level.len() as u64));
            at += level.len().next_multiple_of(16);
        }
        let mut out = vec![0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
        let header = [
            self.format.map_or(0, |format| format.value()),
            1,
            self.width,
            self.height,
            self.depth,
            self.layers,
            self.faces,
            self.levels.len() as u32,
            self.supercompression,
            dfd_at as u32,
            dfd_len as u32,
            0,
            0,
        ];
        for word in header {
            out.extend(word.to_le_bytes());
        }
        out.extend(0u64.to_le_bytes());
        out.extend(0u64.to_le_bytes());
        for (offset, length) in &index {
            out.extend(offset.to_le_bytes());
            out.extend(length.to_le_bytes());
            out.extend(length.to_le_bytes());
        }
        out.extend((dfd_len as u32).to_le_bytes());
        out.extend(&dfd);
        for (level, (offset, _)) in self.levels.iter().zip(&index) {
            out.resize(*offset as usize, 0);
            out.extend(level);
        }
        out
    }
}

fn family_of(format: Format) -> TextureFamily {
    match format {
        Format::BC7_SRGB_BLOCK | Format::BC7_UNORM_BLOCK => TextureFamily::Bc7,
        Format::ASTC_4x4_SRGB_BLOCK | Format::ASTC_4x4_UNORM_BLOCK => TextureFamily::Astc4x4,
        Format::ETC2_R8G8B8A8_SRGB_BLOCK | Format::ETC2_R8G8B8A8_UNORM_BLOCK => TextureFamily::Etc2Rgba8,
        _ => TextureFamily::Rgba8,
    }
}

#[test]
fn every_family_reads_its_srgb_and_linear_formats_with_the_whole_chain_as_stored() {
    let rows = [
        (Format::BC7_SRGB_BLOCK, TextureFamily::Bc7, true),
        (Format::BC7_UNORM_BLOCK, TextureFamily::Bc7, false),
        (Format::ASTC_4x4_SRGB_BLOCK, TextureFamily::Astc4x4, true),
        (Format::ASTC_4x4_UNORM_BLOCK, TextureFamily::Astc4x4, false),
        (Format::ETC2_R8G8B8A8_SRGB_BLOCK, TextureFamily::Etc2Rgba8, true),
        (Format::ETC2_R8G8B8A8_UNORM_BLOCK, TextureFamily::Etc2Rgba8, false),
        (Format::R8G8B8A8_SRGB, TextureFamily::Rgba8, true),
        (Format::R8G8B8A8_UNORM, TextureFamily::Rgba8, false),
    ];
    for (format, family, srgb) in rows {
        let file = Ktx2::chain(format, 256, 7);
        let texture = read_ktx2(&file.bytes()).unwrap_or_else(|error| panic!("{format:?}: {error}"));
        assert_eq!((texture.family(), texture.is_srgb(), texture.side()), (family, srgb, 256), "{format:?}");
        assert_eq!(texture.levels().len(), 9, "{format:?}: 256 down to 1");
        for (level, stored) in texture.levels().iter().zip(&file.levels) {
            assert_eq!(level, stored, "{format:?}: levels kept byte for byte");
        }
    }
    assert_eq!(TextureFamily::Bc7.level_bytes(1, 1), 16, "a mip under one block still takes a whole block");
    assert_eq!(TextureFamily::Rgba8.level_bytes(3, 2), 24);
}

#[test]
fn hostile_ktx2_files_are_refused_with_a_typed_code_and_never_panic() {
    let good = || Ktx2::chain(Format::BC7_SRGB_BLOCK, 256, 1);
    let mut truncated = good().bytes();
    truncated.truncate(200);
    let mut short_level = good();
    short_level.levels[3].pop();
    let mut missing_levels = good();
    missing_levels.levels.truncate(4);
    let rows: Vec<(&str, Vec<u8>, RenderCode)> = vec![
        ("not ktx2", b"PNG, not a texture container at all.....".to_vec(), RenderCode::TextureInvalid),
        ("truncated", truncated, RenderCode::TextureInvalid),
        ("basis universal", Ktx2 { format: None, ..good() }.bytes(), RenderCode::TextureInvalid),
        ("zstd levels", Ktx2 { supercompression: SupercompressionScheme::Zstandard.value(), ..good() }.bytes(), RenderCode::TextureInvalid),
        ("a float format", Ktx2 { format: Some(Format::R16G16B16A16_SFLOAT), ..good() }.bytes(), RenderCode::TextureInvalid),
        ("an array", Ktx2 { layers: 2, ..good() }.bytes(), RenderCode::TextureInvalid),
        ("a cube", Ktx2 { faces: 6, ..good() }.bytes(), RenderCode::TextureInvalid),
        ("a 3d texture", Ktx2 { depth: 4, ..good() }.bytes(), RenderCode::TextureInvalid),
        ("a 1d texture", Ktx2 { height: 0, ..good() }.bytes(), RenderCode::TextureInvalid),
        ("a level shorter than its blocks", short_level.bytes(), RenderCode::TextureInvalid),
        ("a chain that stops above 1 x 1", missing_levels.bytes(), RenderCode::TextureInvalid),
        ("past the texture cap", Ktx2 { width: 1 << 15, height: 1 << 15, ..good() }.bytes(), RenderCode::TextureTooLarge),
    ];
    for (name, bytes, code) in rows {
        let error = read_ktx2(&bytes).err().unwrap_or_else(|| panic!("{name} is refused"));
        assert_eq!(error.code(), code, "{name}: {error}");
    }
}

#[test]
fn the_compression_features_are_renderer_owned_and_each_family_names_its_feature_and_formats() {
    let names: Vec<&str> = RENDERER_FEATURES.iter().map(|(_, name)| *name).collect();
    for (family, feature, name) in [
        (TextureFamily::Bc7, Features::TEXTURE_COMPRESSION_BC, "texture-compression-bc"),
        (TextureFamily::Astc4x4, Features::TEXTURE_COMPRESSION_ASTC, "texture-compression-astc"),
        (TextureFamily::Etc2Rgba8, Features::TEXTURE_COMPRESSION_ETC2, "texture-compression-etc2"),
    ] {
        assert!(names.contains(&name), "{name} is requested whenever offered");
        assert_eq!(family.feature(), feature);
        assert!(family.usable(feature) && !family.usable(Features::empty()), "{family:?}");
        assert!(family.format(TextureKind::Color).is_srgb() && !family.format(TextureKind::Normal).is_srgb(), "{family:?}");
    }
    assert!(TextureFamily::Rgba8.usable(Features::empty()), "every device takes RGBA8");
}

#[test]
fn a_compressed_chain_takes_a_quarter_of_rgba8s_bytes_so_the_desktop_high_arrays_shrink_over_3_5x() {
    for side in TextureClass::ALL.map(TextureClass::side) {
        let rgba8 = MaterialClass { size_px: side, layers: 1 }.bytes();
        assert_eq!(TextureFamily::Rgba8.chain_bytes(side), rgba8, "RGBA8 is the ledger's own model");
        for family in [TextureFamily::Bc7, TextureFamily::Astc4x4, TextureFamily::Etc2Rgba8] {
            let ratio = rgba8 as f64 / family.chain_bytes(side) as f64;
            assert!((3.99..=4.0).contains(&ratio), "{family:?} at {side}: {ratio}");
        }
    }
    let class = DeviceClass::from_wire("desktop-high").expect("class");
    let arrays = material_arrays(class, preset_textures(QualityPreset::High, class));
    let rgba8: u64 = arrays.iter().map(MaterialClass::bytes).sum();
    let bc7: u64 = arrays.iter().map(|array| TextureFamily::Bc7.chain_bytes(array.size_px) * u64::from(array.layers)).sum();
    assert!(rgba8 as f64 / bc7 as f64 >= 3.5, "the ledger's material arrays: {rgba8} -> {bc7}");
}

/// A file the Studio's import step wrote (`tests/conformance/rust-assets-ktx2.test.ts` holds it to
/// the importer's output).
fn import_fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../tests/fixtures/asset-import").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_import_steps_family_files_read_here_with_the_descriptor_the_ktx2_crate_builds_for_their_format() {
    let rows = [
        ("bc", TextureFamily::Bc7, Format::BC7_SRGB_BLOCK),
        ("astc", TextureFamily::Astc4x4, Format::ASTC_4x4_SRGB_BLOCK),
        ("etc2", TextureFamily::Etc2Rgba8, Format::ETC2_R8G8B8A8_SRGB_BLOCK),
        ("rgba8", TextureFamily::Rgba8, Format::R8G8B8A8_SRGB),
    ];
    for (wire, family, format) in rows {
        let bytes = import_fixture(&format!("basis-uastc-64.{wire}.ktx2"));
        let texture = read_ktx2(&bytes).unwrap_or_else(|error| panic!("{wire}: {error}"));
        assert_eq!((texture.family(), texture.is_srgb(), texture.side(), texture.levels().len()), (family, true, 64, 7), "{wire}");
        let reader = ktx2::Reader::new(&bytes[..]).expect("a KTX2 file");
        let expected = ktx2::dfd::Basic::from_format(format).expect("a descriptor").0;
        assert_eq!(reader.basic_dfd(), Some(&expected), "{wire}: the import step's descriptor");
        // The value is NUL-terminated, as KTX 2.0 asks of KTXwriter.
        assert_eq!(reader.writer(), Some("Genex asset import\0"), "{wire}");
    }
}

// ---------------------------------------------------------------------------------------------
// GPU

fn gpu() -> Option<Gpu> {
    let require = std::env::var("GENEX_REQUIRE_METAL").is_ok_and(|v| v == "1");
    match Gpu::headless() {
        Ok(gpu) => {
            if require {
                assert_eq!(gpu.receipt().backend, "metal", "GENEX_REQUIRE_METAL=1 but the adapter is not Metal");
            }
            Some(gpu)
        }
        Err(error) if require => panic!("GENEX_REQUIRE_METAL=1 and no WebGPU-core adapter: {error}"),
        Err(error) => {
            eprintln!("skipped: no WebGPU-core adapter ({error})");
            None
        }
    }
}

/// The bytes of one level of one layer of `texture`, read back block row by block row.
fn read_level(gpu: &Gpu, texture: &wgpu::Texture, family: TextureFamily, layer: u32, level: u32) -> Vec<u8> {
    let px = (texture.width() >> level).max(1);
    let blocks = px.div_ceil(family.block_side());
    let row = blocks * family.block_bytes();
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("level readback"),
        size: u64::from(padded * blocks),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let physical = blocks * family.block_side();
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: level, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(blocks) },
        },
        wgpu::Extent3d { width: physical, height: physical, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("mapped"));
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("readback");
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
    mapped.chunks(padded as usize).flat_map(|line| line[..row as usize].to_vec()).collect()
}

#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_a_ktx2_chain_lands_in_its_class_byte_for_byte_for_every_family_the_adapter_offers() {
    let Some(gpu) = gpu() else { return };
    let class = DeviceClass::from_wire("desktop-high").expect("class");
    let quality = preset_textures(QualityPreset::High, class);
    let rows = [
        (TextureFamily::Bc7, Format::BC7_SRGB_BLOCK, Format::BC7_UNORM_BLOCK),
        (TextureFamily::Astc4x4, Format::ASTC_4x4_SRGB_BLOCK, Format::ASTC_4x4_UNORM_BLOCK),
        (TextureFamily::Etc2Rgba8, Format::ETC2_R8G8B8A8_SRGB_BLOCK, Format::ETC2_R8G8B8A8_UNORM_BLOCK),
    ];
    let mut tested = 0;
    for (family, colour, data) in rows {
        if !family.usable(gpu.device.features()) {
            eprintln!("{family:?}: not offered by this adapter");
            continue;
        }
        tested += 1;
        let mut arrays = ClassArrays::with_family(&gpu.device, class, quality, family);
        assert_eq!(arrays.family(), family);
        let base = Ktx2::chain(colour, 512, 3);
        let base_id = arrays.add_compressed(&gpu.device, &gpu.queue, &read_ktx2(&base.bytes()).expect("colour"), TextureKind::Color).expect("placed");
        let normal = Ktx2::chain(data, 1024, 9);
        let normal_id = arrays.add_compressed(&gpu.device, &gpu.queue, &read_ktx2(&normal.bytes()).expect("data"), TextureKind::Normal).expect("placed");
        assert_eq!((base_id.class(), base_id.layer()), (TextureClass::S512, 0), "{family:?}");
        assert_eq!((normal_id.kind(), normal_id.class()), (TextureKind::Normal, TextureClass::S1024), "{family:?}");
        let texture = arrays.texture(TextureClass::S512, TextureKind::Color);
        assert_eq!(texture.format(), family.format(TextureKind::Color));
        for level in [0, 3, 8, 9] {
            assert_eq!(read_level(&gpu, texture, family, 0, level), base.levels[level as usize], "{family:?} level {level}");
        }
        let reserved: u64 = material_arrays(class, quality).iter().map(MaterialClass::bytes).sum();
        assert!(arrays.allocated_bytes() * 4 <= reserved, "{family:?}: a quarter of the RGBA8 reservation at most");
    }
    assert!(tested > 0, "the adapter offers at least one compressed family");
}

#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_a_larger_chain_drops_its_top_levels_into_a_capped_class_and_rgba8_is_refused_by_compressed_arrays() {
    let Some(gpu) = gpu() else { return };
    if !TextureFamily::Bc7.usable(gpu.device.features()) {
        eprintln!("skipped: no BC on this adapter");
        return;
    }
    let class = DeviceClass::from_wire("mobile-mid").expect("class");
    let quality = preset_textures(QualityPreset::Low, class);
    let mut arrays = ClassArrays::with_family(&gpu.device, class, quality, TextureFamily::Bc7);
    let big = Ktx2::chain(Format::BC7_SRGB_BLOCK, 2048, 5);
    let id = arrays.add_compressed(&gpu.device, &gpu.queue, &read_ktx2(&big.bytes()).expect("big"), TextureKind::Color).expect("placed");
    assert_eq!(id.class(), TextureClass::S512, "capped at 512 on mobile-mid: levels from 512 down");
    let texture = arrays.texture(TextureClass::S512, TextureKind::Color);
    assert_eq!(read_level(&gpu, texture, TextureFamily::Bc7, 0, 0), big.levels[2], "the 512 level of the 2048 chain");
    let rgba = TextureData::rgba8(4, 4, vec![255; 64]).expect("rgba");
    let refused = arrays.add(&gpu.device, &gpu.queue, &rgba, TextureKind::Color).expect_err("BC7 arrays hold BC7 only");
    assert_eq!(refused.code(), RenderCode::TextureInvalid);
    let astc = read_ktx2(&Ktx2::chain(Format::ASTC_4x4_SRGB_BLOCK, 256, 1).bytes()).expect("astc");
    let refused = arrays.add_compressed(&gpu.device, &gpu.queue, &astc, TextureKind::Color).expect_err("another family");
    assert_eq!(refused.code(), RenderCode::TextureInvalid);
    let small: CompressedTexture = read_ktx2(&Ktx2::chain(Format::BC7_SRGB_BLOCK, 64, 1).bytes()).expect("small");
    let refused = arrays.add_compressed(&gpu.device, &gpu.queue, &small, TextureKind::Color).expect_err("below the 256 class");
    assert_eq!(refused.code(), RenderCode::TextureInvalid);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("uploads finish");
}
