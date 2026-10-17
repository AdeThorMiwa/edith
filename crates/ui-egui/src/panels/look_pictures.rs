//! Pictures of looks as textures (fork roadmap F12.7): a look's kept pictures are fetched from the
//! library by id (`look.picture`), and a picture carried inside an assistant view is decoded from the
//! view. Both are cached for the session, so a picture is decoded once, not every frame; one that
//! cannot be had (removed, damaged) is remembered as missing and draws as nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{ColorImage, Context, TextureHandle, TextureId, TextureOptions, Vec2};
use serde_json::json;

use crate::FilmcraftApp;

/// Pictures kept in the cache before it starts again.
const MAX_CACHED: usize = 240;

#[derive(Default)]
struct Cache {
    stills: HashMap<(String, String), Option<TextureHandle>>,
    inline: HashMap<u64, Option<TextureHandle>>,
}

type Shared = Arc<Mutex<Cache>>;

fn shared(ctx: &Context) -> Shared {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("look-pictures"), || Arc::new(Mutex::new(Cache::default()))).clone())
}

/// A picture as pixels, from the base64 of its JPEG.
fn decode(b64: &str) -> Option<ColorImage> {
    let bytes = filmcraft_project::gtemplate::base64_decode(b64)?;
    let r = filmcraft_media::still::decode_raster("look.jpg", &bytes).ok()?;
    let (w, h) = (r.width as usize, r.height as usize);
    (w > 0 && h > 0 && w <= 2048 && h <= 2048 && r.rgba.len() >= w * h * 4).then(|| ColorImage::from_rgba_unmultiplied([w, h], &r.rgba))
}

fn handle(ctx: &Context, name: &str, img: ColorImage) -> (TextureHandle, Vec2) {
    let size = egui::vec2(img.size[0] as f32, img.size[1] as f32);
    (ctx.load_texture(name, img, TextureOptions::LINEAR), size)
}

/// A kept picture of a look, by ids.
pub fn still(app: &mut FilmcraftApp, ctx: &Context, look: &str, still: &str) -> Option<(TextureId, Vec2)> {
    let cache = shared(ctx);
    let key = (look.to_string(), still.to_string());
    if let Some(known) = cache.lock().unwrap_or_else(PoisonError::into_inner).stills.get(&key) {
        return known.as_ref().map(|t| (t.id(), t.size_vec2()));
    }
    let made = app
        .session
        .execute("look.picture", json!({"look": look, "still": still}))
        .ok()
        .and_then(|v| v["jpegBase64"].as_str().and_then(decode))
        .map(|img| handle(ctx, &format!("look-{look}-{still}"), img));
    let out = made.as_ref().map(|(t, s)| (t.id(), *s));
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if c.stills.len() + c.inline.len() >= MAX_CACHED {
        c.stills.clear();
        c.inline.clear();
    }
    c.stills.insert(key, made.map(|(t, _)| t));
    out
}

/// A picture carried in an assistant view.
pub fn inline(ctx: &Context, b64: &str) -> Option<(TextureId, Vec2)> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b64.hash(&mut h);
    let key = h.finish();
    let cache = shared(ctx);
    if let Some(known) = cache.lock().unwrap_or_else(PoisonError::into_inner).inline.get(&key) {
        return known.as_ref().map(|t| (t.id(), t.size_vec2()));
    }
    let made = decode(b64).map(|img| handle(ctx, &format!("look-inline-{key}"), img));
    let out = made.as_ref().map(|(t, s)| (t.id(), *s));
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if c.stills.len() + c.inline.len() >= MAX_CACHED {
        c.stills.clear();
        c.inline.clear();
    }
    c.inline.insert(key, made.map(|(t, _)| t));
    out
}
