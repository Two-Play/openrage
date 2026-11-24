//! The flying camera of the model and map views: W A S D and Q E move it,
//! dragging looks around, the wheel sets how fast it is.

use std::collections::HashSet;
use std::time::Instant;

use openrage_app::render::{self, Camera};

const MOVE_KEYS: [&str; 6] = ["w", "a", "s", "d", "q", "e"];
/// With Shift.
const FAST: f32 = 4.0;
/// How much a notch of the wheel changes the speed.
const NOTCH: f32 = 1.25;
/// Metres a second.
const SLOWEST: f32 = 0.02;
const FASTEST: f32 = 20_000.0;

pub struct Fly {
    pub eye: [f32; 3],
    yaw: f32,
    pitch: f32,
    /// Metres a second.
    speed: f32,
    /// Movement keys held down (lower-case), and whether Shift is held.
    keys: HashSet<String>,
    fast: bool,
    last_tick: Instant,
}

impl Fly {
    /// Starts where a camera is; `size` is how large what it looks at is.
    pub fn new(camera: &Camera, size: f32) -> Self {
        let f = camera.forward;
        Self {
            eye: camera.eye,
            yaw: f[1].atan2(f[0]),
            pitch: f[2].clamp(-1.0, 1.0).asin(),
            speed: (size * 0.35).clamp(SLOWEST, FASTEST),
            keys: HashSet::new(),
            fast: false,
            last_tick: Instant::now(),
        }
    }

    pub fn camera(&self, radius: f32) -> Camera {
        render::fly_camera(self.eye, self.yaw, self.pitch, radius)
    }

    /// Dragging, in pixels.
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.005;
        self.pitch = (self.pitch - dy * 0.005).clamp(-1.55, 1.55);
    }

    /// The wheel; `delta` is 120 for a notch.
    pub fn wheel(&mut self, delta: f32) {
        self.speed = (self.speed * NOTCH.powf(delta / 120.0)).clamp(SLOWEST, FASTEST);
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(SLOWEST, FASTEST);
    }

    /// How fast it is, to show.
    pub fn speed_text(&self) -> String {
        match self.speed {
            v if v >= 1000.0 => format!("{:.1} km/s", v / 1000.0),
            v if v >= 10.0 => format!("{v:.0} m/s"),
            v => format!("{v:.2} m/s"),
        }
    }

    /// Whether the key moves the camera.
    pub fn key_down(&mut self, key: &str, shift: bool) -> bool {
        self.fast = shift;
        let moves = MOVE_KEYS.contains(&key);
        if moves {
            if self.keys.is_empty() {
                self.last_tick = Instant::now();
            }
            self.keys.insert(key.to_owned());
        }
        moves
    }

    /// Whether a key is still held.
    pub fn key_up(&mut self, key: &str, shift: bool) -> bool {
        self.fast = shift;
        self.keys.remove(key);
        !self.keys.is_empty()
    }

    pub fn release(&mut self) {
        self.keys.clear();
    }

    /// Moves as long as the time since the last call.
    pub fn tick(&mut self) {
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        let held = |k: &str| f32::from(u8::from(self.keys.contains(k)));
        let (ahead, side, up) = (held("w") - held("s"), held("d") - held("a"), held("e") - held("q"));
        let camera = self.camera(1.0);
        let step = self.speed * if self.fast { FAST } else { 1.0 } * dt;
        for k in 0..3 {
            let up = if k == 2 { up } else { 0.0 };
            self.eye[k] += (camera.forward[k] * ahead + camera.right[k] * side + up) * step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fly() -> Fly {
        Fly::new(&render::fly_camera([1.0, 2.0, 3.0], 0.5, -0.2, 10.0), 100.0)
    }

    #[test]
    fn it_starts_where_the_camera_was() {
        let from = render::fly_camera([1.0, 2.0, 3.0], 0.5, -0.2, 10.0);
        let now = fly().camera(10.0);
        assert_eq!(now.eye, from.eye);
        assert!((0..3).all(|k| (now.forward[k] - from.forward[k]).abs() < 1e-5));
    }

    #[test]
    fn the_wheel_changes_the_speed() {
        let mut f = fly();
        let before = f.speed;
        f.wheel(120.0);
        assert!((f.speed / before - NOTCH).abs() < 1e-5);
        (0..500).for_each(|_| f.wheel(120.0));
        assert_eq!(f.speed, FASTEST);
        (0..500).for_each(|_| f.wheel(-120.0));
        assert_eq!(f.speed, SLOWEST);
    }

    #[test]
    fn keys() {
        let mut f = fly();
        assert!(!f.key_down("x", false));
        assert!(f.key_down("w", false) && f.key_down("a", true));
        assert!(f.key_up("w", false));
        assert!(!f.key_up("a", false));
    }
}
