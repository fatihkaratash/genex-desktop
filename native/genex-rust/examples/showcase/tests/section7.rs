//! The 4K and 1440p capture plan (renderer pass 2, plan section 7): the showcase scenes' settled
//! Studio captures at 2560x1440 and 3840x2160, high and ultra. G0 (2026-10-15) found no tooling
//! wrote them and the determinism row covered only the template at one preset.
//!
//! Metal rows (`--ignored metal`), one at a time:
//!
//!   GENEX_REQUIRE_METAL=1 node scripts/toolchains/rust-env.ts -- cargo test -p showcase \
//!     --release --offline --locked --test section7 -- --ignored metal --test-threads=1
//!
//! - The determinism row captures the Lakeside's wide shot and Harbour Street's street-wide shot
//!   at both presets and sizes in four child processes each, and requires one digest.
//! - The stills row writes every shot of both scenes at both presets and sizes as
//!   `<scene>/<shot>-<preset>-<W>x<H>.png` with `digests.json` into `GENEX_SECTION7_OUT` (default
//!   `pass2/captures/latest` in the evidence folder), each taken twice in one process and required
//!   to draw one picture. Crops and A/B sheets are cut from these PNGs (ffmpeg).

mod support;

use std::path::PathBuf;
use std::process::Command;

use genex::present::Presenter;
use genex::render::{PresetChoice, RenderChange};
use genex::{FIXED_STEP_MS, Runtime, View};
use genex_render::capture::Capture;
use genex_render::settings::QualityPreset;
use support::{SHOT_TICK, gpu, lakeside_at_shot_tick, write_png};

/// The plan's output sizes.
const SIZES: [(u32, u32); 2] = [(2560, 1440), (3840, 2160)];
/// The plan's presets at those sizes.
const PRESETS: [QualityPreset; 2] = [QualityPreset::High, QualityPreset::Ultra];
/// Child processes per capture in the determinism row.
const CHILD_RUNS: usize = 4;
/// The shots the determinism row captures, by scene.
const DETERMINISM_SHOTS: [(Scene, &str); 2] = [(Scene::Lakeside, "wide"), (Scene::Harbour, "street-wide")];
/// What a child run captures (`scene,shot,preset,WxH`) and where it writes the digest.
const CHILD_ENV: &str = "GENEX_SECTION7_CHILD";
const CHILD_OUT_ENV: &str = "GENEX_SECTION7_CHILD_OUT";
const OUT_ENV: &str = "GENEX_SECTION7_OUT";

/// The showcase's scenes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    Lakeside,
    Harbour,
}

impl Scene {
    fn wire(self) -> &'static str {
        match self {
            Scene::Lakeside => "lakeside",
            Scene::Harbour => "harbour-street",
        }
    }

    fn from_wire(wire: &str) -> Option<Scene> {
        [Scene::Lakeside, Scene::Harbour].into_iter().find(|scene| scene.wire() == wire)
    }

    fn shots(self) -> Vec<&'static str> {
        match self {
            Scene::Lakeside => showcase_game::world::SHOTS.iter().map(|shot| shot.name).collect(),
            Scene::Harbour => showcase_game::harbour::SHOTS.iter().map(|shot| shot.name).collect(),
        }
    }
}

/// `shot` of `scene` at tick 600, captured the way the Studio captures it at `preset` and `size`.
fn capture(scene: Scene, shot: &str, preset: QualityPreset, size: (u32, u32)) -> Option<Capture> {
    match scene {
        Scene::Lakeside => {
            let presenter = Presenter::open(showcase_game::showcase::WORLD_SIZE, showcase_game::assets()).ok()?;
            Some(settled(presenter, lakeside_at_shot_tick(), shot, preset, size))
        }
        Scene::Harbour => {
            let presenter = Presenter::open(showcase_game::harbour::WORLD_SIZE, showcase_game::harbour_assets()).ok()?;
            let mut runtime = Runtime::new(showcase_game::harbour_game()).expect("installs");
            runtime.seed(1);
            runtime.start();
            runtime.step(f64::from(SHOT_TICK) * FIXED_STEP_MS);
            Some(settled(presenter, runtime, shot, preset, size))
        }
    }
}

/// A settled capture of `shot` on `presenter` at `preset` and `size`.
fn settled<W: Default + 'static>(mut presenter: Presenter, mut runtime: Runtime<W>, shot: &str, preset: QualityPreset, size: (u32, u32)) -> Capture {
    assert!(presenter.install_for(&mut runtime).is_empty(), "every pass installs");
    presenter.set_capturing(true);
    presenter.set_capture_size(Some(size));
    // The first frame attaches the game's render settings.
    presenter.draw_view(&mut runtime, View::Named(shot)).expect("a view").expect("drawn");
    let change = RenderChange { preset: Some(Some(PresetChoice::Preset(preset))), ..RenderChange::default() };
    presenter.render_settings().expect("the game's settings attached").borrow_mut().apply(&change);
    presenter.draw_view(&mut runtime, View::Named(shot)).expect("a view").expect("drawn");
    assert!(presenter.take_pass_refusals().is_empty(), "{shot}: every pass draws");
    let capture = presenter.resilient.capture().expect("capture");
    assert_eq!((capture.width(), capture.height()), size, "{shot}: captured at the size asked for");
    capture
}

fn parse_size(text: &str) -> (u32, u32) {
    let (w, h) = text.split_once('x').expect("WxH");
    (w.parse().expect("width"), h.parse().expect("height"))
}

/// The child half of the determinism row: with [`CHILD_ENV`] set, capture what it names and write
/// the digest to [`CHILD_OUT_ENV`].
#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_section7_child_capture() {
    let (Ok(what), Ok(out)) = (std::env::var(CHILD_ENV), std::env::var(CHILD_OUT_ENV)) else { return };
    let [scene, shot, preset, size] = what.split(',').collect::<Vec<_>>().try_into().expect("scene,shot,preset,WxH");
    let scene = Scene::from_wire(scene).expect("a showcase scene");
    let preset = QualityPreset::from_wire(preset).expect("a preset");
    let shot = capture(scene, shot, preset, parse_size(size)).expect("a GPU in the child");
    std::fs::write(out, format!("{:016x}", shot.digest())).expect("child digest");
}

fn child_digest(what: &str, run: usize) -> String {
    let out = std::env::temp_dir().join(format!("genex-section7-child-{}-{run}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let status = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--ignored", "--exact", "metal_section7_child_capture", "--test-threads=1"])
        .env(CHILD_ENV, what)
        .env(CHILD_OUT_ENV, &out)
        .status()
        .expect("child run");
    assert!(status.success(), "child run {run} of {what} failed");
    let digest = std::fs::read_to_string(&out).expect("the child wrote its digest");
    let _ = std::fs::remove_file(&out);
    digest
}

/// Section 7 and gate G0: the showcase's settled captures at 1440p and 4K, high and ultra, are the
/// same bytes in four processes (the template's row covered one preset of the template only).
#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_showcase_captures_at_1440p_and_4k_are_deterministic_across_processes() {
    if gpu().is_none() {
        return;
    }
    let mut problems = Vec::new();
    for (scene, shot) in DETERMINISM_SHOTS {
        for preset in PRESETS {
            for (w, h) in SIZES {
                let what = format!("{},{shot},{},{w}x{h}", scene.wire(), preset.wire());
                let digests: Vec<String> = (0..CHILD_RUNS).map(|run| child_digest(&what, run)).collect();
                eprintln!("{what}: {digests:?}");
                if digests.iter().any(|digest| *digest != digests[0]) {
                    problems.push(format!("{what} differs across processes: {digests:?}"));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Section 7's stills: every shot of both scenes at high and ultra at 1440p and 4K, each taken twice
/// in one process (one picture), written as PNGs with their digests.
#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_section7_stills_are_written_at_1440p_and_4k() {
    if gpu().is_none() {
        return;
    }
    let root = std::env::var_os(OUT_ENV).map_or_else(|| support::evidence_dir("pass2/captures/latest"), PathBuf::from);
    let mut rows = Vec::new();
    for scene in [Scene::Lakeside, Scene::Harbour] {
        let dir = root.join(scene.wire());
        std::fs::create_dir_all(&dir).expect("the stills folder");
        for shot in scene.shots() {
            for preset in PRESETS {
                for (w, h) in SIZES {
                    let first = capture(scene, shot, preset, (w, h)).expect("a presenter");
                    let again = capture(scene, shot, preset, (w, h)).expect("a second presenter");
                    assert_eq!(again.digest(), first.digest(), "{} {shot} {} {w}x{h}: two settled captures differ", scene.wire(), preset.wire());
                    write_png(&dir.join(format!("{shot}-{}-{w}x{h}.png", preset.wire())), &first);
                    rows.push(format!("{{\"scene\": \"{}\", \"shot\": \"{shot}\", \"preset\": \"{}\", \"size\": [{w}, {h}], \"digest\": \"{:016x}\"}}", scene.wire(), preset.wire(), first.digest()));
                }
            }
        }
    }
    std::fs::write(root.join("digests.json"), format!("{{\"tick\": {SHOT_TICK}, \"rows\": [\n{}\n]}}\n", rows.join(",\n"))).expect("digests");
}
