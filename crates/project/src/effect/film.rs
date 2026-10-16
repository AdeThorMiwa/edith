//! The Film Look pack (fork roadmap F07): effects that make clean footage read as film. Names are
//! generic on purpose (no studio or film-stock trademarks). Pixel implementations live in
//! `filmcraft_render::vfx::film`.

use super::*;

/// The Effects-panel folder of the pack.
pub(super) const FILM: &[&str] = &["Video Effects", "Film Look"];

/// Grain stocks: choice 0 uses the controls; the others set size, softness and the brightness
/// response (all in the same units as the controls): (size, softness, shadows, midtones, highlights).
pub const GRAIN_STOCKS: &[&str] = &["Custom", "Fine", "Medium", "Heavy"];
pub const GRAIN_STOCK_VALUES: [(f64, f64, f64, f64, f64); 3] = [(1.0, 25.0, 60.0, 100.0, 40.0), (1.8, 35.0, 70.0, 100.0, 55.0), (3.2, 45.0, 85.0, 100.0, 70.0)];

fn pctf(id: &'static str, label: &'static str, def: f64) -> ParamDef {
    f(id, label, def, 0.0, 100.0, "%")
}

pub(super) fn defs() -> Vec<EffectDef> {
    vec![video(
        "film_grain",
        "Film Grain",
        FILM,
        vec![
            pctf("amount", "Amount", 25.0),
            ch("stock", "Stock", GRAIN_STOCKS, 0),
            fs("size", "Grain Size", 1.6, (0.5, 12.0), (0.5, 6.0), "px", 1),
            pctf("softness", "Softness", 30.0),
            pctf("color", "Color Grain", 20.0),
            grp(pctf("shadows", "Shadows", 70.0), "Brightness Response"),
            grp(pctf("midtones", "Midtones", 100.0), "Brightness Response"),
            grp(pctf("highlights", "Highlights", 50.0), "Brightness Response"),
            fs("seed", "Random Seed", 0.0, (0.0, 10000.0), (0.0, 1000.0), "", 0),
        ],
    )]
}
