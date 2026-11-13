//! Probe: render a ped's head close up (parts whose name contains a word).
fn main() {
    let game = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let ped = std::env::args().nth(2).unwrap();
    let outfit: usize = std::env::args().nth(3).unwrap().parse().unwrap();
    let out = std::env::args().nth(4).unwrap();
    let yaw: f32 = std::env::args().nth(5).map_or(0.0, |y| y.parse().unwrap());
    let store =
        rage_rpf::rpf8_keys::KeyStore::load(&rage_rpf::rpf8_keys::KeyStore::default_path().unwrap()).unwrap().unwrap();
    let names = rage_rpf::rpf8_names::NameIndex::load(&rage_rpf::rpf8_names::NameIndex::default_path(&game).unwrap())
        .unwrap()
        .map(std::sync::Arc::new);
    if let Some(n) = &names {
        openrage_app::rdr2::register_names(n);
    }
    rage_rpf::rpf8::set_defaults(Some(std::sync::Arc::new(store.to_keys().unwrap())), names);
    let model = openrage_app::models::load_rdr2_ped(&game, &ped, outfit).unwrap();
    if std::env::var_os("DIFF").is_some() {
        unsafe { std::env::set_var("PROBE_NO_SHAPES", "1") };
        let plain = openrage_app::models::load_rdr2_ped(&game, &ped, outfit).unwrap();
        for ((n, a), (_, b)) in model.drawables.iter().zip(&plain.drawables) {
            let (mut moved, mut max, mut total, mut nan) = (0, 0f32, 0, 0);
            for (ma, mb) in a.meshes.iter().zip(&b.meshes) {
                for (p, q) in ma.positions.iter().zip(&mb.positions) {
                    total += 1;
                    if p.iter().any(|x| !x.is_finite()) {
                        nan += 1;
                        continue;
                    }
                    let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                    if d > 1e-4 {
                        moved += 1;
                    }
                    max = max.max(d);
                }
            }
            println!("{n}: {moved}/{total} moved, max {max:.4} m, {nan} NaN");
        }
        unsafe { std::env::remove_var("PROBE_NO_SHAPES") };
    }
    let mut look = model.default_look();
    // Only parts near the head: those whose bounds top is in the upper tenth.
    let (lo, hi) = openrage_app::render::bounds(&model, &look.parts).unwrap();
    let top = hi[2] - (hi[2] - lo[2]) * 0.13;
    let (_, hlo_hi) = (lo, hi);
    let _ = hlo_hi;
    let materials = openrage_app::render::Materials::new(&model);
    let view = openrage_app::render::View { yaw: yaw.to_radians(), ..Default::default() };
    // Frame the head: a box around the top of the model.
    let cam_lo = [lo[0] + (hi[0] - lo[0]) * 0.3, lo[1] + (hi[1] - lo[1]) * 0.3, top];
    let cam_hi = [hi[0] - (hi[0] - lo[0]) * 0.3, hi[1] - (hi[1] - lo[1]) * 0.3, hi[2]];
    let camera = openrage_app::render::orbit_camera(cam_lo, cam_hi, view);
    look.parts.iter_mut().for_each(|_| {});
    let rgba = openrage_app::render::render_camera(&model, &look, &materials, &camera, None, 600, 600);
    std::fs::write(out, rage_formats::png(600, 600, &rgba).unwrap()).unwrap();
}
