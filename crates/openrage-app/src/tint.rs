//! RDR2 tint palettes: colour textures that hold a shade and two masks
//! instead of colours, coloured from three rows of a palette.
//!
//! A tinted albedo (flagged in `assets_albedo.ymt`) keeps the shade in green
//! and the masks for the second and third tint in red and blue. A palette
//! (`metaped_tint_*` in `textures/graphics.ytd`) is 64 pixels wide, one row
//! per tint; columns 8..64 run from dark to light, and the shade picks the
//! column. The first tint colours everything, the second where red is set,
//! the third where blue is set. Tint 255 leaves its layer out.

/// The first palette column of the shade ramp (columns 0..8 are the same in every row).
const RAMP_START: f32 = 8.0;

/// Colours an albedo (RGBA, as decoded) with three palette rows.
/// `palette` is RGBA, `width` × `height`.
pub fn bake(albedo: &[u8], palette: &[u8], width: usize, height: usize, tints: [u8; 3]) -> Vec<u8> {
    let ramp = (width as f32 - 1.0 - RAMP_START).max(0.0);
    let sample = |tint: u8, shade: f32| -> Option<[f32; 3]> {
        if tint == 255 || width == 0 || height == 0 {
            return None;
        }
        let row = (tint as usize).min(height - 1);
        let x = RAMP_START.min(width as f32 - 1.0) + shade * ramp;
        let (x0, t) = (x.floor() as usize, x.fract());
        let x1 = (x0 + 1).min(width - 1);
        let px = |x: usize, c: usize| palette[(row * width + x) * 4 + c] as f32;
        Some([0, 1, 2].map(|c| px(x0, c) * (1.0 - t) + px(x1, c) * t))
    };
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|c| a[c] * (1.0 - t) + b[c] * t);
    let mut out = Vec::with_capacity(albedo.len());
    for px in albedo.as_chunks::<4>().0 {
        let (red, shade, blue) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        let mut c = sample(tints[0], shade).unwrap_or([px[1] as f32; 3]);
        if let Some(second) = sample(tints[1], shade) {
            c = mix(c, second, red);
        }
        if let Some(third) = sample(tints[2], shade) {
            c = mix(c, third, blue);
        }
        out.extend(c.map(|v| v.round().clamp(0.0, 255.0) as u8));
        out.push(px[3]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_picks_the_column_masks_pick_the_row() {
        // Two-row palette, 64 wide: row 0 ramps red, row 1 ramps blue.
        let (w, h) = (64usize, 2usize);
        let mut pal = vec![0u8; w * h * 4];
        for x in 0..w {
            let v = (x * 4).min(255) as u8;
            pal[x * 4] = v;
            pal[(w + x) * 4 + 2] = v;
            pal[x * 4 + 3] = 255;
            pal[(w + x) * 4 + 3] = 255;
        }
        // Full shade, no masks: the last column of the first tint's row.
        let out = bake(&[0, 255, 0, 200], &pal, w, h, [0, 1, 1]);
        assert_eq!(out, vec![252, 0, 0, 200]);
        // Red mask: the second tint's row.
        let out = bake(&[255, 255, 0, 255], &pal, w, h, [0, 1, 255]);
        assert_eq!(out, vec![0, 0, 252, 255]);
        // Darkest shade: the start of the ramp (column 8).
        let out = bake(&[0, 0, 0, 255], &pal, w, h, [0, 255, 255]);
        assert_eq!(out, vec![32, 0, 0, 255]);
    }
}
