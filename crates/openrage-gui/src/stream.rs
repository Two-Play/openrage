//! Loading the details of GTA VI's map around the camera, in the background.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};

use openrage_app::gta6_world::{Close, Loaded, Lod, Quality};
use openrage_app::models::Look;
use openrage_app::render;
use rage_formats::math::Mat4;
use rayon::prelude::*;

use crate::ModelPreview;

/// Models loaded before the picture is renewed.
const AT_ONCE: usize = 96;
/// Map files read before the picture is renewed.
const FILES_AT_ONCE: usize = 200;
/// How many models that are not drawn are kept.
const KEPT: usize = 1500;

/// Models with their places, as a scene draws them.
pub type Drawn = Vec<(Arc<ModelPreview>, Look, Vec<Mat4>)>;

/// Nearer than this, models get their best textures (metres).
const UP_CLOSE: f32 = 70.0;

/// Where the camera is, and how far things are in detail.
type From = ([f32; 3], f32);

#[derive(Default)]
struct Wish {
    from: Option<From>,
    stop: bool,
}

/// The thread that loads; ends when this is dropped.
pub struct Stream {
    wish: Arc<(Mutex<Wish>, Condvar)>,
    last: Option<From>,
}

impl Stream {
    /// Starts with the models of the farthest level. `show` gets what to draw
    /// and whether there is more to come; it is called from the thread.
    pub fn start(
        lod: Arc<Lod>,
        far: HashMap<u32, Arc<ModelPreview>>,
        show: impl Fn(Drawn, bool) -> bool + Send + 'static,
    ) -> Self {
        let wish = Arc::new((Mutex::new(Wish::default()), Condvar::new()));
        let theirs = Arc::clone(&wish);
        std::thread::spawn(move || work(&lod, far, &theirs, &show));
        Self { wish, last: None }
    }

    /// The camera is here now; things are in detail `range` times as far as
    /// in the game.
    pub fn look_from(&mut self, eye: [f32; 3], range: f32) {
        // Small steps change nothing.
        let moved = |(from, r): &From| *r != range || (0..3).map(|k| (from[k] - eye[k]).powi(2)).sum::<f32>() > 4.0;
        if self.last.as_ref().is_none_or(moved) {
            self.last = Some((eye, range));
            let (lock, changed) = &*self.wish;
            lock.lock().unwrap_or_else(|e| e.into_inner()).from = Some((eye, range));
            changed.notify_one();
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let (lock, changed) = &*self.wish;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        changed.notify_one();
    }
}

struct Kept {
    preview: Arc<ModelPreview>,
    look: Look,
    /// When it was drawn last.
    drawn: u64,
    /// Always kept.
    far: bool,
    /// With its best textures.
    best: bool,
}

/// How a model looks in a scene: with water and lamps told from the rest.
pub fn scene_look(preview: &ModelPreview) -> Look {
    let mut look = preview.loaded.default_look();
    let is_water = preview.name.to_ascii_lowercase().contains("water");
    for (slot, l) in preview.loaded.materials.iter().zip(look.materials.iter_mut()) {
        let shader = rage_formats::names::value(slot.name_hash);
        if shader.starts_with("water") || (!slot.textured && is_water) {
            l.tint = [0.16, 0.3, 0.36, crate::gpu::WATER];
        } else if !slot.textured {
            // A colour the game computes; what lies on others would hide them.
            l.visible = !shader.contains("decal");
            l.tint = [0.45, 0.45, 0.44, 1.0];
        } else if shader.contains("emissive") {
            l.tint[3] = crate::gpu::LAMP;
        }
    }
    look
}

/// Whether a placement is near the camera.
fn near_to(eye: &[f32; 3], place: &Mat4) -> bool {
    (0..3).map(|k| (place[12 + k] - eye[k]).powi(2)).sum::<f32>() < UP_CLOSE * UP_CLOSE
}

/// Models ready to be drawn. What can be done here is not done while a
/// picture is drawn.
pub fn ready(loaded: Vec<Loaded>) -> Vec<(u32, Arc<ModelPreview>)> {
    let prepare = |(m, name, mut loaded): Loaded| {
        let materials = render::Materials::new(&loaded);
        let look = std::sync::RwLock::new(loaded.default_look());
        loaded.forget_shared();
        (m, Arc::new(ModelPreview { loaded, look, materials, name }))
    };
    let (keys, previews): (Vec<u32>, Vec<Arc<ModelPreview>>) = loaded.into_par_iter().map(prepare).unzip();
    crate::gpu::prepare(&previews);
    keys.into_iter().zip(previews).collect()
}

fn work(
    lod: &Lod,
    far: HashMap<u32, Arc<ModelPreview>>,
    wish: &(Mutex<Wish>, Condvar),
    show: &dyn Fn(Drawn, bool) -> bool,
) {
    let keep =
        |preview: Arc<ModelPreview>, far| Kept { look: scene_look(&preview), preview, drawn: 0, far, best: false };
    let mut models: HashMap<u32, Kept> = far.into_iter().map(|(m, p)| (m, keep(p, true))).collect();
    // Models that cannot be read are not tried again.
    let mut failed: HashSet<u32> = HashSet::new();
    let mut close: HashMap<u32, Close> = HashMap::new();
    let mut from = None;
    let mut turn = 0u64;
    let mut more = false;
    loop {
        {
            let (lock, changed) = wish;
            let mut now = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if now.stop {
                    return;
                }
                if let Some(wanted) = now.from.take() {
                    from = Some(wanted);
                    break;
                }
                if more {
                    break;
                }
                now = changed.wait(now).unwrap_or_else(|e| e.into_inner());
            }
        }
        let Some((eye, range)) = from else { continue };
        turn += 1;
        let near = lod.near(&eye, range);
        close.retain(|file, _| near.contains(file));
        let unread: Vec<u32> = near.iter().filter(|f| !close.contains_key(f)).copied().take(FILES_AT_ONCE).collect();
        let files_left = !unread.is_empty();
        close.extend(lod.read(&unread));
        // A file that cannot be read stays away from the next round, too.
        for file in unread {
            close.entry(file).or_default();
        }
        let cut = lod.cut(&eye, range, &|m| models.contains_key(&m), &|m| failed.contains(&m), close.values());
        let mut better: Vec<u32> = Vec::new();
        let drawn: Drawn = cut
            .draw
            .into_iter()
            .filter_map(|(m, places)| {
                let kept = models.get_mut(&m)?;
                kept.drawn = turn;
                if !kept.best && places.iter().any(|t| near_to(&eye, t)) {
                    better.push(m);
                }
                Some((Arc::clone(&kept.preview), kept.look.clone(), places))
            })
            .collect();
        let wanted: Vec<u32> = cut.wanted.into_iter().take(AT_ONCE).collect();
        better.truncate(if wanted.is_empty() { AT_ONCE } else { 8 });
        more = files_left || !wanted.is_empty() || !better.is_empty();
        if !show(drawn, more) {
            return;
        }
        // The ones not drawn for longest go first.
        let mut idle: Vec<(u64, u32)> =
            models.iter().filter(|(_, k)| !k.far && k.drawn < turn).map(|(m, k)| (k.drawn, *m)).collect();
        if idle.len() > KEPT {
            idle.sort_unstable();
            for (_, m) in &idle[..idle.len() - KEPT] {
                models.remove(m);
            }
        }
        let quiet = &|_, _| {};
        let loaded = ready(lod.load(&wanted, Quality::Near, quiet));
        for (m, preview) in ready(lod.load(&better, Quality::Close, quiet)) {
            if let Some(kept) = models.get_mut(&m) {
                (kept.look, kept.preview, kept.best) = (scene_look(&preview), preview, true);
            }
        }
        // Those that could not be read again are not tried for ever.
        for m in &better {
            if let Some(kept) = models.get_mut(m) {
                kept.best = true;
            }
        }
        failed.extend(wanted.iter().copied());
        for (m, preview) in loaded {
            failed.remove(&m);
            models.insert(m, Kept { drawn: turn, ..keep(preview, false) });
        }
    }
}
