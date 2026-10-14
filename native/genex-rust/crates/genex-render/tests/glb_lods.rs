//! Import-time LODs (renderer pass 2, A6.9): a GLB part carries the `genex_lods` levels its
//! primitives brought (`extras.genex_lods`, the Studio's import step), validated, rebased into the
//! part's vertices and merged level by level, each error scaled by its node; a refused set is a
//! typed warning and the mesh still loads whole. The level a draw takes is the coarsest whose
//! error projects to at most the threshold in pixels, the same on the CPU and in WGSL.
//!
//! The GPU row is `#[ignore]`d and its name contains `metal`:
//!
//!   GENEX_REQUIRE_METAL=1 node scripts/toolchains/rust-env.ts -- cargo test -p genex-render \
//!     --offline --locked --test glb_lods -- --ignored metal

use genex_render::mesh::{GlbModel, LOD_MAX_LEVELS, LOD_SELECT_WGSL, LodFault, MeshData, select_lod};
use genex_render::Gpu;
use serde_json::{Value, json};

/// The repository's asset-import fixtures.
fn import_fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../tests/fixtures/asset-import").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A GLB's JSON and binary chunks.
fn split(glb: &[u8]) -> (Value, Vec<u8>) {
    let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("length")) as usize;
    let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).expect("json");
    let bin_at = 20 + json_len;
    let bin_len = u32::from_le_bytes(glb[bin_at..bin_at + 4].try_into().expect("length")) as usize;
    (json, glb[bin_at + 8..bin_at + 8 + bin_len].to_vec())
}

/// `json` and `bin` as a GLB.
fn join(json: &Value, mut bin: Vec<u8>) -> Vec<u8> {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut text = serde_json::to_vec(json).expect("json");
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut out = Vec::new();
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend(((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(&text);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(&bin);
    out
}

/// The golden with `edit` applied to its JSON.
fn edited(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let (mut json, bin) = split(&import_fixture("lods-sphere.glb"));
    edit(&mut json);
    join(&json, bin)
}

#[test]
fn the_import_steps_golden_loads_with_each_level_merged_across_its_primitives() {
    let bytes = import_fixture("lods-sphere.glb");
    let (json, _) = split(&bytes);
    let primitives = json["meshes"][0]["primitives"].as_array().expect("primitives");
    let levels: Vec<&Vec<Value>> = primitives.iter().map(|p| p["extras"]["genex_lods"]["levels"].as_array().expect("levels")).collect();
    let model = GlbModel::from_glb(&bytes).expect("the golden loads");
    let [part] = model.parts() else { panic!("one part: the sphere names no material") };
    assert!(part.lod_warnings.is_empty(), "{:?}", part.lod_warnings);
    let chain = part.lods.as_ref().expect("a chain");
    assert_eq!(chain.levels.len(), levels.iter().map(|l| l.len()).min().unwrap_or(0));
    for (at, level) in chain.levels.iter().enumerate() {
        let count: u64 = levels.iter().map(|l| l[at]["count"].as_u64().expect("count")).sum();
        assert_eq!(level.indices.len() as u64, count, "level {at}: every primitive's triangles");
        let error = levels.iter().map(|l| l[at]["error"].as_f64().expect("error") as f32).fold(0.0_f32, f32::max);
        assert_eq!(level.error, error, "level {at}: the largest of its primitives' errors");
        assert!(level.indices.iter().all(|&i| (i as usize) < part.mesh.vertices.len()), "level {at}: rebased into the part");
    }
    assert!(chain.levels.windows(2).all(|w| w[1].indices.len() <= w[0].indices.len() && w[1].error >= w[0].error));
    assert!(chain.levels[0].indices.len() < part.mesh.indices.len(), "level 1 is coarser than the mesh itself");
    let mesh = MeshData::from_glb(&bytes).expect("the plain loader still reads it");
    assert_eq!(mesh.indices.len(), part.mesh.indices.len());
}

#[test]
fn a_scaled_node_scales_its_levels_error() {
    let plain = GlbModel::from_glb(&import_fixture("lods-sphere.glb")).expect("loads");
    let scaled = GlbModel::from_glb(&edited(|json| json["nodes"][0]["scale"] = json!([2.0, 2.0, 2.0]))).expect("loads");
    let errors = |model: &GlbModel| model.parts()[0].lods.as_ref().expect("chain").levels.iter().map(|l| l.error).collect::<Vec<f32>>();
    let doubled: Vec<f32> = errors(&plain).iter().map(|e| e * 2.0).collect();
    assert_eq!(errors(&scaled), doubled);
}

#[test]
fn hostile_lod_extras_are_refused_with_a_typed_warning_and_the_mesh_still_loads_whole() {
    type Damage = fn(&mut Value);
    let rows: [(&str, LodFault, Damage); 8] = [
        ("another version", LodFault::UnknownVersion, |l| l["version"] = json!(99)),
        ("not an object", LodFault::UnknownVersion, |l| *l = json!("levels")),
        ("too many levels", LodFault::OverLimit, |l| {
            let first = l["levels"][0].clone();
            l["levels"] = json!(vec![first; LOD_MAX_LEVELS as usize + 1]);
        }),
        ("a view past the buffer", LodFault::RangeOutOfBounds, |l| l["indices"] = json!(999)),
        ("a level past its view", LodFault::RangeOutOfBounds, |l| l["levels"][0]["count"] = json!(3_000_000_000_u64)),
        ("a level that is not whole triangles", LodFault::NotCoarser, |l| l["levels"][0]["count"] = json!(l["levels"][0]["count"].as_u64().unwrap_or(3) - 1)),
        ("a finer level after a coarser one", LodFault::NotCoarser, |l| {
            let mut levels = l["levels"].as_array().cloned().unwrap_or_default();
            levels.reverse();
            l["levels"] = json!(levels);
        }),
        ("a negative error", LodFault::NotCoarser, |l| l["levels"][0]["error"] = json!(-1.0)),
    ];
    for (name, fault, damage) in rows {
        let bytes = edited(|json| damage(&mut json["meshes"][0]["primitives"][0]["extras"]["genex_lods"]));
        let model = GlbModel::from_glb(&bytes).unwrap_or_else(|error| panic!("{name}: the mesh loads ({error})"));
        let part = &model.parts()[0];
        assert!(part.lods.is_none(), "{name}: no chain");
        assert_eq!(part.lod_warnings.iter().map(|w| (w.primitive, w.fault)).collect::<Vec<_>>(), vec![(0, fault)], "{name}");
    }
}

#[test]
fn an_index_past_the_primitives_vertices_is_refused_vertex_out_of_mesh() {
    let positions: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0];
    let mut bin: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
    bin.extend([0u16, 1, 2, 2, 1, 3].iter().flat_map(|i| i.to_le_bytes()));
    bin.extend([0u32, 1, 7].iter().flat_map(|i| i.to_le_bytes()));
    let json = json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": [{"buffer": 0, "byteLength": 48}, {"buffer": 0, "byteOffset": 48, "byteLength": 12}, {"buffer": 0, "byteOffset": 60, "byteLength": 12}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
            {"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"}
        ],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1,
            "extras": {"genex_lods": {"version": 1, "indices": 2, "levels": [{"first": 0, "count": 3, "error": 0.1}]}}}]}],
        "nodes": [{"mesh": 0}], "scenes": [{"nodes": [0]}], "scene": 0
    });
    let model = GlbModel::from_glb(&join(&json, bin)).expect("loads");
    assert!(model.parts()[0].lods.is_none());
    assert_eq!(model.parts()[0].lod_warnings[0].fault, LodFault::VertexOutOfMesh);
    assert_eq!(model.parts()[0].lod_warnings[0].fault.wire(), "lods-vertex-out-of-mesh");
}

#[test]
fn a_part_with_one_primitive_lacking_levels_has_no_chain_and_no_warning() {
    let bytes = edited(|json| {
        if let Some(extras) = json["meshes"][0]["primitives"][1]["extras"].as_object_mut() {
            extras.remove("genex_lods");
        }
    });
    let model = GlbModel::from_glb(&bytes).expect("loads");
    assert!(model.parts()[0].lods.is_none() && model.parts()[0].lod_warnings.is_empty());
}

#[test]
fn the_coarsest_level_whose_error_projects_within_the_threshold_is_drawn() {
    let errors = [0.01, 0.05, 0.2];
    // 1000 pixels a world unit at distance 1, a 1-pixel threshold.
    let rows = [(5.0, 0), (20.0, 1), (60.0, 2), (250.0, 3), (1_000.0, 3), (0.0, 0), (-3.0, 0)];
    for (distance, level) in rows {
        assert_eq!(select_lod(&errors, distance, 1000.0, 1.0), level, "distance {distance}");
    }
    assert_eq!(select_lod(&[], 1e9, 1000.0, 1.0), 0, "no chain: the mesh itself");
    assert_eq!(select_lod(&[0.0], 0.001, 1000.0, 1.0), 1, "an exact level is taken even up close");
    assert_eq!(select_lod(&errors, f32::NAN, 1000.0, 1.0), 0, "a non-finite distance draws the mesh itself");
}

/// The WGSL select inside a compute entry that runs it once per case.
fn select_shader() -> String {
    format!(
        "{LOD_SELECT_WGSL}\nstruct Case {{ errors: vec4<f32>, levels: u32, distance: f32, pixels_per_unit: f32, threshold_px: f32 }}\n\
         @group(0) @binding(0) var<storage, read> cases: array<Case>;\n\
         @group(0) @binding(1) var<storage, read_write> picked: array<u32>;\n\
         @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
           if (id.x >= arrayLength(&cases)) {{ return; }}\n\
           let c = cases[id.x];\n\
           picked[id.x] = gx_lod_select(c.errors, c.levels, c.distance, c.pixels_per_unit, c.threshold_px);\n\
         }}\n"
    )
}

#[test]
fn the_wgsl_select_validates_on_webgpu_core_and_emits_hlsl_msl_and_spirv() {
    use naga::back::{hlsl, msl, spv};
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    let module = naga::front::wgsl::parse_str(&select_shader()).expect("WGSL parses");
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty()).validate(&module).expect("WebGPU core");
    let mut hlsl_source = String::new();
    let hlsl_options = hlsl::Options { shader_model: hlsl::ShaderModel::V5_1, ..Default::default() };
    hlsl::Writer::new(&mut hlsl_source, &hlsl_options, &hlsl::PipelineOptions::default()).write(&module, &info, None).expect("HLSL SM 5.1");
    let msl_options = msl::Options { lang_version: (2, 4), ..Default::default() };
    msl::write_string(&module, &info, &msl_options, &msl::PipelineOptions::default()).expect("MSL 2.4");
    let spv_options = spv::Options { lang_version: (1, 3), ..Default::default() };
    spv::write_vec(&module, &info, &spv_options, None).expect("SPIR-V 1.3");
}

// ---------------------------------------------------------------------------------------------
// GPU

fn gpu() -> Option<Gpu> {
    let require = std::env::var("GENEX_REQUIRE_METAL").is_ok_and(|v| v == "1");
    match Gpu::headless() {
        Ok(gpu) => Some(gpu),
        Err(error) if require => panic!("GENEX_REQUIRE_METAL=1 and no WebGPU-core adapter: {error}"),
        Err(error) => {
            eprintln!("skipped: no WebGPU-core adapter ({error})");
            None
        }
    }
}

/// One case: four errors (unused ones past `levels`), the level count, distance, pixels a unit, threshold.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Case {
    errors: [f32; 4],
    levels: u32,
    distance: f32,
    pixels_per_unit: f32,
    threshold_px: f32,
}

/// Deterministic cases around every boundary: errors rising, distances on a log scale and at the
/// exact distances each error crosses the threshold.
fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let mut seed = 0x2545_f491_u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32) / (u32::MAX as f32)
    };
    for _ in 0..512 {
        let levels = 1 + (next() * 4.0) as u32 % 4;
        let mut errors = [0.0_f32; 4];
        let mut running = 0.0;
        for error in errors.iter_mut().take(levels as usize) {
            running += next() * 0.1;
            *error = running;
        }
        let pixels_per_unit = 200.0 + next() * 2000.0;
        let threshold_px = 0.5 + next() * 2.0;
        let distance = libm::powf(10.0, next() * 4.0 - 1.0);
        out.push(Case { errors, levels, distance, pixels_per_unit, threshold_px });
        let crossing = errors[0] * pixels_per_unit / threshold_px;
        out.push(Case { errors, levels, distance: crossing, pixels_per_unit, threshold_px });
    }
    out
}

#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_wgsl_lod_select_picks_the_cpus_level_for_every_case() {
    let Some(gpu) = gpu() else { return };
    let cases = cases();
    let source = select_shader();
    let device = &gpu.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("lod select"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let input = wgpu::util::DeviceExt::create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor { label: Some("cases"), contents: bytemuck::cast_slice(&cases), usage: wgpu::BufferUsages::STORAGE },
    );
    let size = (cases.len() * 4) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor { label: Some("picked"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("staging"), size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("lod select"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
        ],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((cases.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, size);
    gpu.queue.submit([encoder.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("mapped"));
    device.poll(wgpu::PollType::wait_indefinitely()).expect("done");
    let picked: Vec<u32> = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().expect("mapped range")).to_vec();
    let mut changes = 0;
    for (at, case) in cases.iter().enumerate() {
        let cpu = select_lod(&case.errors[..case.levels as usize], case.distance, case.pixels_per_unit, case.threshold_px);
        assert_eq!(picked[at], cpu, "case {at}: {:?} at {}", &case.errors[..case.levels as usize], case.distance);
        changes += usize::from(cpu > 0 && cpu < case.levels);
    }
    assert!(changes > 50, "the cases cross level boundaries ({changes})");
}
