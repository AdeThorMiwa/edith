//! Colour maths for the measures: sRGB to CIE L*a*b* (D65), distance, and correlated colour
//! temperature (McCamy 1992, from the chromaticity of the average colour).

/// The D65 white point (CIE 1931, 2 degree observer).
const WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];

/// sRGB-encoded 0..1 to linear light (the IEC 61966-2-1 curve). Out-of-range input is clamped.
pub fn srgb_to_linear(v: f32) -> f32 {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Linear sRGB (Rec.709 primaries) to CIE XYZ, Y = 1 for white.
pub fn linear_to_xyz([r, g, b]: [f32; 3]) -> [f32; 3] {
    [0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b, 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b, 0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b]
}

fn lab_f(t: f32) -> f32 {
    const D: f32 = 6.0 / 29.0;
    if t > D * D * D { t.cbrt() } else { t / (3.0 * D * D) + 4.0 / 29.0 }
}

/// sRGB-encoded 0..1 to CIE L*a*b* (L 0..100, a and b about -128..127).
pub fn srgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let lin = [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2])];
    let xyz = linear_to_xyz(lin);
    let (fx, fy, fz) = (lab_f(xyz[0] / WHITE[0]), lab_f(xyz[1] / WHITE[1]), lab_f(xyz[2] / WHITE[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The distance between two Lab colours (CIE76: plain Euclidean).
pub fn lab_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dl, da, db) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dl * dl + da * da + db * db).sqrt()
}

/// The correlated colour temperature in kelvin of a linear-light colour (McCamy's cubic from the
/// chromaticity). `None` for black or a colour whose chromaticity is outside what the cubic
/// describes. The result is limited to 1000..40000 K.
pub fn colour_temperature(linear_rgb: [f32; 3]) -> Option<f32> {
    let [x, y, z] = linear_to_xyz(linear_rgb);
    let sum = x + y + z;
    if !(sum.is_finite() && sum > 1e-6) {
        return None;
    }
    let (cx, cy) = (x / sum, y / sum);
    let denom = 0.1858 - cy;
    if denom.abs() < 1e-4 {
        return None;
    }
    let n = (cx - 0.3320) / denom;
    let cct = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
    cct.is_finite().then(|| cct.clamp(1000.0, 40000.0))
}
