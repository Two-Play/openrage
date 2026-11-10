//! Minimal 4×4 matrix helpers (column-major, like glTF and GPU shaders).

pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Translation × rotation (quaternion `x, y, z, w`) × scale.
pub fn from_trs(t: [f32; 3], q: [f32; 4], s: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = q;
    let len = (x * x + y * y + z * z + w * w).sqrt();
    let (x, y, z, w) = if len > 1e-6 { (x / len, y / len, z / len, w / len) } else { (0.0, 0.0, 0.0, 1.0) };
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz, wx, wy, wz) = (x * y, x * z, y * z, w * x, w * y, w * z);
    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        (2.0 * (xy + wz)) * s[0],
        (2.0 * (xz - wy)) * s[0],
        0.0,
        (2.0 * (xy - wz)) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        (2.0 * (yz + wx)) * s[1],
        0.0,
        (2.0 * (xz + wy)) * s[2],
        (2.0 * (yz - wx)) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// Rotation about the Z axis (the game's up axis).
pub fn rotation_z(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    [c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// `a × b` (apply `b` first).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Transforms a direction (no translation); fine for normals under rigid transforms.
pub fn transform_dir(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trs_and_mul() {
        let quarter = std::f32::consts::FRAC_PI_4.sin();
        // 90° about Z, then move by (1, 2, 3).
        let m = from_trs([1.0, 2.0, 3.0], [0.0, 0.0, quarter, quarter], [1.0; 3]);
        let p = transform_point(&m, [1.0, 0.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-5 && (p[1] - 3.0).abs() < 1e-5 && (p[2] - 3.0).abs() < 1e-5, "{p:?}");
        let r = mul(&rotation_z(std::f32::consts::PI), &IDENTITY);
        let q = transform_point(&r, [1.0, 0.0, 0.0]);
        assert!((q[0] + 1.0).abs() < 1e-5 && q[1].abs() < 1e-5);
    }
}
