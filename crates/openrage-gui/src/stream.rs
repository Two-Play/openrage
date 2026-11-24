//! Loading the details of GTA VI's map around the camera, in the background.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};

use openrage_app::gta6_world::{Close, Lod};
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

#[derive(Default)]
struct Wish {
    eye: Option<[f32; 3]>,
    stop: bool,
}

/// The thread that loads; ends when this is dropped.
pub struct Stream {
    wish: Arc<(Mutex<Wish>, Condvar)>,
    last: Option<[f32; 3]>,
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

    /// The camera is here now.
    pub fn look_from(&mut self, eye: [f32; 3]) {
        // Small steps change nothing.
        let moved = |from: &[f32; 3]| (0..3).map(|k| (from[k] - eye[k]).powi(2)).sum::<f32>() > 4.0;
        if self.last.as_ref().is_none_or(moved) {
            self.last = Some(eye);
            let (lock, changed) = &*self.wish;
            lock.lock().unwrap_or_else(|e| e.into_inner()).eye = Some(eye);
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
}

/// How a model looks in a scene: without decals, which need blending it does not do.
pub fn scene_look(preview: &ModelPreview) -> Look {
    let mut look = preview.loaded.default_look();
    for (slot, l) in preview.loaded.materials.iter().zip(look.materials.iter_mut()) {
        if rage_formats::shaders::name(slot.name_hash).is_some_and(|n| n.contains("decal")) {
            l.visible = false;
        }
    }
    look
}

fn work(
    lod: &Lod,
    far: HashMap<u32, Arc<ModelPreview>>,
    wish: &(Mutex<Wish>, Condvar),
    show: &dyn Fn(Drawn, bool) -> bool,
) {
    let keep = |preview: Arc<ModelPreview>, far| Kept { look: scene_look(&preview), preview, drawn: 0, far };
    let mut models: HashMap<u32, Kept> = far.into_iter().map(|(m, p)| (m, keep(p, true))).collect();
    // Models that cannot be read are not tried again.
    let mut failed: HashSet<u32> = HashSet::new();
    let mut close: HashMap<u32, Close> = HashMap::new();
    let mut eye = None;
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
                if let Some(wanted) = now.eye.take() {
                    eye = Some(wanted);
                    break;
                }
                if more {
                    break;
                }
                now = changed.wait(now).unwrap_or_else(|e| e.into_inner());
            }
        }
        let Some(eye) = eye else { continue };
        turn += 1;
        let near = lod.near(&eye);
        close.retain(|file, _| near.contains(file));
        let unread: Vec<u32> = near.iter().filter(|f| !close.contains_key(f)).copied().take(FILES_AT_ONCE).collect();
        let files_left = !unread.is_empty();
        close.extend(lod.read(&unread));
        // A file that cannot be read stays away from the next round, too.
        for file in unread {
            close.entry(file).or_default();
        }
        let cut = lod.cut(&eye, &|m| models.contains_key(&m), &|m| failed.contains(&m), close.values());
        let drawn: Drawn = cut
            .draw
            .into_iter()
            .filter_map(|(m, places)| {
                let kept = models.get_mut(&m)?;
                kept.drawn = turn;
                Some((Arc::clone(&kept.preview), kept.look.clone(), places))
            })
            .collect();
        let wanted: Vec<u32> = cut.wanted.into_iter().take(AT_ONCE).collect();
        more = files_left || !wanted.is_empty();
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
        let loaded: Vec<(u32, Arc<ModelPreview>)> = lod
            .load(&wanted, false, &|_, _| {})
            .into_par_iter()
            .map(|(m, name, loaded)| {
                let materials = render::Materials::new(&loaded);
                let look = std::sync::RwLock::new(loaded.default_look());
                (m, Arc::new(ModelPreview { loaded, look, materials, name }))
            })
            .collect();
        failed.extend(wanted.iter().copied());
        for (m, preview) in loaded {
            failed.remove(&m);
            models.insert(m, Kept { drawn: turn, ..keep(preview, false) });
        }
    }
}
