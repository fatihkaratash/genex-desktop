//! Harbour Street's five fixed shots (renderer pass 2, A.13 blockout) captured on the host GPU (Metal
//! on the Mac) the way the Studio captures them: genex's presenter on its own device with the
//! game's look (AgX punchy, auto exposure, the damp air's fog, bloom), a settled capture at tick 600
//! (ten seconds after seed 1). Each shot is drawn twice to one digest, written as
//! `harbour-<shot>.png`, held to a sane exposure (not black, not blown), and its digest to the
//! committed per-adapter row (`GENEX_RENDER_GOLDEN_UPDATE=1` rewrites this adapter's row). The scene
//! draws its fetched CC0 scans where the asset cache holds them (`GENEX_SHOWCASE_FETCHED=0` draws
//! its untextured stand-ins); each is its own picture with its own row.
//!
//! The rows are `#[ignore]`d and named `metal`:
//!
//!   GENEX_REQUIRE_METAL=1 node scripts/toolchains/rust-env.ts -- cargo test -p showcase --release \
//!     --offline --locked --test harbour_shots -- --ignored metal --nocapture
//!
//! `GENEX_SHOWCASE_OUT` names the folder (default `.studio-dev/evidence/rust-renderer/captures/latest`).

mod support;

use genex::{FIXED_STEP_MS, Runtime, View};
use genex_render::capture::Capture;
use showcase_game::harbour::{SHOTS, WORLD_SIZE, World};
use showcase_game::{harbour_assets, harbour_game};
use support::{SHOT_TICK, gpu, out_dir, percentile, write_png};

/// A shot is exposed when its mean encoded luma (0..255) lies in this range and no more than this
/// share of its pixels clip to white.
const EXPOSED_LUMA: std::ops::RangeInclusive<f64> = 30.0..=200.0;
const CLIPPED_LUMA: f64 = 250.0;
const CLIPPED_SHARE_MAX: f64 = 0.02;
/// The window-shafts shot looks at the sun's patches on the café floor: its brightest 1% stands
/// at least this many times over its median (no patch, no shafts).
const SHAFT_CONTRAST_MIN: f64 = 1.8;

/// Harbour Street at tick 600 of seed 1.
fn harbour_at_shot_tick() -> Runtime<World> {
    let mut runtime = Runtime::new(harbour_game()).expect("installs");
    runtime.seed(1);
    runtime.start();
    runtime.step(f64::from(SHOT_TICK) * FIXED_STEP_MS);
    runtime
}

/// `name` at tick 600 captured the way the Studio captures it, or None without a GPU.
fn studio_shot(name: &str) -> Option<Capture> {
    use genex::present::Presenter;
    let mut presenter = Presenter::open(WORLD_SIZE, harbour_assets()).ok()?;
    let mut runtime = harbour_at_shot_tick();
    let refused = presenter.install_for(&mut runtime);
    assert!(refused.is_empty(), "{refused:?}");
    presenter.set_capturing(true);
    presenter.draw_view(&mut runtime, View::Named(name)).expect("a view").expect("drawn");
    let path = presenter.resilient.renderer().renderer().last_scene().expect("a frame");
    assert!(path.hdr, "{name}: the Studio draws the HDR scene: {path:?}");
    Some(presenter.resilient.capture().expect("capture"))
}

/// Encoded luma (BT.709, 0..255) of every pixel of `capture`, ascending.
fn lumas(capture: &Capture) -> Vec<f64> {
    let mut lumas: Vec<f64> = capture.bgra().chunks(4).map(|p| 0.2126 * f64::from(p[2]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[0])).collect();
    lumas.sort_by(f64::total_cmp);
    lumas
}

/// The committed per-adapter digests of the shots (fetched and procedural rows).
fn check_digests(digests: &[(&str, String)]) {
    let Some(adapter) = gpu().map(|gpu| gpu.receipt().adapter.clone()) else { return };
    let fetched = harbour_assets().iter().any(|(name, _)| name.starts_with("fetched/"));
    let adapter = format!("{adapter} ({})", if fetched { "fetched" } else { "procedural" });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../tests/fixtures/render-goldens/harbour-street/studio-digests.json");
    let read = std::fs::read_to_string(&path).ok().and_then(|text| text.parse::<genex::Value>().ok());
    let mut golden = read.unwrap_or_else(|| genex::json!({}));
    let mut shots = genex::json!({});
    for (name, digest) in digests {
        shots[*name] = genex::json!(digest);
    }
    if std::env::var("GENEX_RENDER_GOLDEN_UPDATE").is_ok_and(|v| v == "1") {
        golden[&adapter] = shots;
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("goldens folder");
        std::fs::write(&path, format!("{golden:#}\n")).expect("golden written");
        return;
    }
    match golden.get(&adapter) {
        Some(expected) => assert_eq!(expected, &shots, "Harbour Street's digests on {adapter} changed: name them and rewrite with GENEX_RENDER_GOLDEN_UPDATE=1"),
        None => eprintln!("skipped: no committed Harbour Street digests for {adapter}"),
    }
}

#[test]
#[ignore = "needs a GPU: run with --ignored metal"]
fn metal_every_harbour_shot_is_drawn_the_same_twice_exposed_and_written_as_a_png() {
    if gpu().is_none() {
        return;
    }
    let dir = out_dir();
    let mut digests = Vec::new();
    let mut failures = Vec::new();
    for shot in SHOTS {
        let Some(capture) = studio_shot(shot.name) else { return };
        let again = studio_shot(shot.name).expect("a second presenter");
        write_png(&dir.join(format!("harbour-{}.png", shot.name)), &capture);
        assert_eq!(again.digest(), capture.digest(), "{}: two settled captures of one tick differ", shot.name);
        let lumas = lumas(&capture);
        let mean = lumas.iter().sum::<f64>() / lumas.len() as f64;
        let clipped = lumas.iter().filter(|luma| **luma >= CLIPPED_LUMA).count() as f64 / lumas.len() as f64;
        let (median, top) = (percentile(&lumas, 0.5), percentile(&lumas, 0.99));
        println!("{}: mean luma {mean:.1}, clipped {:.2}%, median {median:.1}, p99 {top:.1}", shot.name, clipped * 100.0);
        if !EXPOSED_LUMA.contains(&mean) {
            failures.push(format!("{}: mean luma {mean:.1} outside {EXPOSED_LUMA:?}", shot.name));
        }
        if clipped > CLIPPED_SHARE_MAX {
            failures.push(format!("{}: {:.2}% of the pixels clip", shot.name, clipped * 100.0));
        }
        if shot.name == "window-shafts" && top < SHAFT_CONTRAST_MIN * median {
            failures.push(format!("window-shafts: p99 {top:.1} under {SHAFT_CONTRAST_MIN}x the median {median:.1}: no sun patch"));
        }
        digests.push((shot.name, format!("{:016x}", capture.digest())));
    }
    let listed: Vec<String> = digests.iter().map(|(name, digest)| format!("\"{name}\": \"{digest}\"")).collect();
    let record = format!("{{\"tick\": {SHOT_TICK}, \"path\": \"presenter capture\", \"digests\": {{{}}}}}\n", listed.join(", "));
    std::fs::write(dir.join("harbour-digests.json"), &record).expect("digests");
    println!("{record}");
    assert!(failures.is_empty(), "{} exposure checks failed:\n{}", failures.len(), failures.join("\n"));
    check_digests(&digests);
}
