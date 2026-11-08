//! Turning `.zoo` object records into drawable sprites.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::Vec3;
use zt_formats::zoo::{ObjectRecord, ZooMap};
use zt_formats::{Ani, Ini, Palette, Sprite, sprite};
use zt_render::sprites::{
    AtlasBuilder, AtlasRect, FLAG_FLIP, FLAG_PLANE_H, FLAG_PLANE_U, FLAG_PLANE_V, FLAG_TRANSLUCENT, SpriteInstance,
};
use zt_vfs::Vfs;

/// One frame in the atlas.
#[derive(Clone, Copy, Debug)]
pub struct FrameRef {
    pub rect: AtlasRect,
    /// Top-left corner relative to the anchor.
    pub offset: [f32; 2],
}

/// All directions of one animation.
#[derive(Debug)]
pub struct Anim {
    /// Per direction (`.ani` order): frames and frame time.
    pub dirs: Vec<(Vec<FrameRef>, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// NE, SE, SW, NW views selected by facing (fences, scenery).
    FourWay,
    /// N, NE, E, SE, S views; west-facing views are mirrored (units).
    FiveWay,
    /// Path tiles: variant chosen from neighbours and slope.
    Path,
    Single,
    /// A vertical face on a tile edge (tank water sides, glass): direction 0
    /// is the `\` view, 1 the `/` view. `along_v`: the edge runs along v
    /// (constant u).
    Face { along_v: bool },
}

pub struct Drawable {
    pub pos: Vec3,
    pub anim: Rc<Anim>,
    pub kind: Kind,
    pub facing: u32,
    pub phase: u32,
    /// Bias toward the camera in tile units (larger for big footprints).
    pub depth_bias: f32,
    /// Sort tier: ground decals first.
    pub tier: u8,
    /// Path connectivity [-v, +u, +v, -u] and screen-space slope info.
    pub path_links: [bool; 4],
    pub path_corners: [i32; 4],
    /// Drawn at 50% opacity.
    pub translucent: bool,
    /// Transient effects: visible for `active` of every `period` ms (offset
    /// by `phase`), restarting the animation each time.
    pub schedule: Option<Schedule>,
    /// Depth from a plane instead of one value for the whole sprite.
    pub plane: Plane,
    /// Underwater things: blended 50% toward this water colour.
    pub tint: Option<[f32; 3]>,
    /// Swimmers: the surface swim and its underwater half; the water module
    /// switches to them for animals at a tank's surface.
    pub swim: Option<(Rc<Anim>, Option<Rc<Anim>>)>,
    /// Added to the painter's sort key (things that reach over their
    /// neighbours, like a tank gate's platform).
    pub sort_bias: f32,
}

/// Depth mode of a sprite. Billboards use one depth (anchor + bias); the
/// others are flat things (tank walls, water) whose depth follows their plane,
/// so the terrain hides exactly the part below the ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    Billboard,
    /// Vertical, on a tile edge; `along_v`: the edge runs along v.
    Edge { along_v: bool },
    Ground,
}

impl Plane {
    fn flags(self) -> u32 {
        match self {
            Plane::Billboard => 0,
            Plane::Edge { along_v: true } => FLAG_PLANE_U,
            Plane::Edge { along_v: false } => FLAG_PLANE_V,
            Plane::Ground => FLAG_PLANE_H,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    pub period: u32,
    pub active: u32,
}

impl Drawable {
    /// A drawable with the defaults for everything but position and graphics.
    pub fn new(pos: Vec3, anim: Rc<Anim>, kind: Kind, facing: u32, seed: u32) -> Drawable {
        Drawable {
            pos,
            anim,
            kind,
            facing,
            phase: hash(seed) % 100_000,
            depth_bias: 1.25,
            tier: 1,
            path_links: [false; 4],
            path_corners: [0; 4],
            translucent: false,
            schedule: None,
            plane: Plane::Billboard,
            tint: None,
            swim: None,
            sort_bias: 0.0,
        }
    }
}

/// Cheap integer hash for deterministic per-object variation.
pub fn hash(x: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ (h >> 12)
}

/// Which side of the tile a scenery/fence facing refers to, in the
/// NE, SE, SW, NW order of the `.ani` files. Index = facing / 2.
///
/// Fences sit on the edge given by their position: facing 0 is the tile's
/// −v edge (upper left on screen at rotation 0), 2 the +u edge (upper right),
/// 4 the +v edge, 6 the −u edge.
pub const FOUR_WAY_FROM_FACING: [usize; 4] = [3, 0, 1, 2];

pub struct ObjectSet {
    pub drawables: Vec<Drawable>,
    pub atlas: AtlasBuilder,
    pub missing: Vec<String>,
}

pub struct Loader<'a> {
    pub vfs: &'a Vfs,
    atlas: AtlasBuilder,
    anims: HashMap<String, Option<Rc<Anim>>>,
    palettes: HashMap<String, Option<[[u8; 4]; 256]>>,
    scenery_ai: HashMap<String, String>,
}

impl Loader<'_> {
    fn palette(&mut self, path: &str) -> Option<[[u8; 4]; 256]> {
        if let Some(p) = self.palettes.get(path) {
            return *p;
        }
        let p = self.vfs.read(path).ok().and_then(|b| Palette::parse(&b).ok()).map(|p| p.rgba256());
        self.palettes.insert(path.to_string(), p);
        p
    }

    pub fn anim(&mut self, ani_path: &str) -> Option<Rc<Anim>> {
        if let Some(a) = self.anims.get(ani_path) {
            return a.clone();
        }
        let a = self.load_anim(ani_path).map(Rc::new);
        if a.is_none() {
            log::debug!("missing animation {ani_path}");
        }
        self.anims.insert(ani_path.to_string(), a.clone());
        a
    }

    fn load_anim(&mut self, ani_path: &str) -> Option<Anim> {
        let ani = Ani::parse(&self.vfs.read_text(ani_path).ok()?);
        let mut dirs = Vec::new();
        for i in 0..ani.animations.len() {
            let sp = ani.sprite_path(i)?;
            let s = Sprite::parse(&self.vfs.read(&sp).ok()?).ok()?;
            let pal = self.palette(&s.palette_path)?;
            let mut frames = Vec::new();
            // FATZ backgrounds are drawn under every frame; merge them in.
            for f in &s.frames {
                frames.push(self.add_frame(f, s.background.as_ref(), &pal));
            }
            dirs.push((frames, s.frame_ms.max(1)));
        }
        Some(Anim { dirs })
    }

    fn add_frame(&mut self, f: &sprite::Frame, bg: Option<&sprite::Frame>, pal: &[[u8; 4]; 256]) -> FrameRef {
        // Combined bounds relative to the anchor.
        let rect_of = |f: &sprite::Frame| {
            let (x0, y0) = (-(f.anchor_x as i32), -(f.anchor_y as i32));
            (x0, y0, x0 + f.width as i32, y0 + f.height as i32)
        };
        let mut b = rect_of(f);
        if let Some(bg) = bg {
            let r = rect_of(bg);
            b = (b.0.min(r.0), b.1.min(r.1), b.2.max(r.2), b.3.max(r.3));
        }
        let (w, h) = ((b.2 - b.0).max(1) as u32, (b.3 - b.1).max(1) as u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for layer in bg.into_iter().chain(std::iter::once(f)) {
            if layer.mask {
                continue; // underwater shading masks: not drawn yet
            }
            let (lx, ly, _, _) = rect_of(layer);
            let px = layer.decode();
            for (j, &p) in px.iter().enumerate() {
                if p == sprite::TRANSPARENT || p == sprite::MASKED {
                    continue;
                }
                let x = (lx - b.0) as u32 + (j % layer.width as usize) as u32;
                let y = (ly - b.1) as u32 + (j / layer.width as usize) as u32;
                let c = pal[p as usize];
                let o = ((y * w + x) * 4) as usize;
                rgba[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let rect = self.atlas.add(w, h, &rgba);
        FrameRef { rect, offset: [b.0 as f32, b.1 as f32] }
    }

    pub fn ai(&self, path: &str) -> Option<Ini> {
        Some(Ini::parse(&self.vfs.read_text(path).ok()?))
    }

    /// Animation name for `key` (e.g. "idle") from an `.ai`, honouring a
    /// subtype prefix like `[m/Animations]`.
    pub fn anim_name(ai: Option<&Ini>, sub: Option<&str>, key: &str) -> String {
        let Some(ai) = ai else { return key.to_string() };
        let sec = sub.map(|s| format!("{s}/Animations"));
        sec.as_deref()
            .and_then(|s| ai.get(s, key))
            .or_else(|| ai.get("Animations", key))
            .map(|s| s.to_string())
            .unwrap_or_else(|| key.to_string())
    }

    /// Two single-view animations as the two directions of a [`Kind::Face`].
    pub fn face_anim(&mut self, backslash: &str, slash: &str) -> Option<Rc<Anim>> {
        let key = format!("{backslash}|{slash}");
        if let Some(a) = self.anims.get(&key) {
            return a.clone();
        }
        let a = (|| {
            let a = self.anim(backslash)?;
            let b = self.anim(slash)?;
            Some(Rc::new(Anim { dirs: vec![a.dirs.first()?.clone(), b.dirs.first()?.clone()] }))
        })();
        self.anims.insert(key, a.clone());
        a
    }

    fn footprint(ai: Option<&Ini>) -> f32 {
        let Some(ai) = ai else { return 1.0 };
        let fx = ai.get_int("Characteristics/Integers", "cFootprintX").unwrap_or(1);
        let fy = ai.get_int("Characteristics/Integers", "cFootprintY").unwrap_or(1);
        fx.max(fy).max(1) as f32
    }

    /// Average colour (0–1) of the first frame of an animation's first view.
    pub fn average_color(&mut self, ani_path: &str) -> Option<[f32; 3]> {
        let ani = Ani::parse(&self.vfs.read_text(ani_path).ok()?);
        let s = Sprite::parse(&self.vfs.read(&ani.sprite_path(0)?).ok()?).ok()?;
        let pal = self.palette(&s.palette_path)?;
        let (mut sum, mut n) = ([0f32; 3], 0f32);
        for p in s.frames.first()?.decode() {
            if p == sprite::TRANSPARENT || p == sprite::MASKED {
                continue;
            }
            let c = pal[p as usize];
            for i in 0..3 {
                sum[i] += c[i] as f32 / 255.0;
            }
            n += 1.0;
        }
        (n > 0.0).then(|| sum.map(|v| v / n))
    }

    /// A swimmer's surface swim (`surfswim`) and the underwater half drawn
    /// with it (`undersurfswim`), from its `.ai`.
    fn swim_anims(&mut self, o: &ObjectRecord) -> Option<(Rc<Anim>, Option<Rc<Anim>>)> {
        if o.group != "animals" {
            return None;
        }
        let t = o.type_name.as_str();
        let sub = o.subtype.as_deref();
        let ai = self.ai(&format!("animals/{t}.ai"))?;
        let bases: Vec<String> = match sub {
            Some(s) => vec![format!("animals/{t}/{s}"), format!("animals/{t}/m")],
            None => vec![format!("animals/{t}")],
        };
        let find = |l: &mut Self, key: &str| {
            let name = Self::anim_name(Some(&ai), sub, key);
            if name == key && ai.get("Animations", key).is_none() && sub.is_none_or(|s| ai.get(&format!("{s}/Animations"), key).is_none()) {
                return None;
            }
            bases.iter().find_map(|b| l.anim(&format!("{b}/{name}/{name}.ani"))).filter(|a| !anim_is_empty(a))
        };
        let surf = find(self, "surfswim")?;
        Some((surf, find(self, "undersurfswim")))
    }

    /// Resolve a record to (animation, kind, footprint).
    fn resolve(&mut self, o: &ObjectRecord) -> Option<(Rc<Anim>, Kind, f32)> {
        let t = o.type_name.as_str();
        let sub = o.subtype.as_deref();
        match o.group.as_str() {
            "fences" => {
                let sub = sub.unwrap_or("f");
                let ai = self.ai(&format!("fences/{t}.ai"));
                let idle = Self::anim_name(ai.as_ref(), Some(sub), "idle");
                let a = self.anim(&format!("fences/{t}/{sub}/{idle}/{idle}.ani"))?;
                Some((a, Kind::FourWay, 1.0))
            }
            "objects" | "building" => {
                let ai = self.scenery_ai.get(t).cloned().and_then(|p| self.ai(&p));
                let idle = Self::anim_name(ai.as_ref(), sub, "idle");
                // Either a folder name under the object, or a full path
                // (`twaterwv` borrows `objects/waterwv1/idle/idle.ani`).
                let path = if idle.ends_with(".ani") { idle } else { format!("objects/{t}/{idle}/{idle}.ani") };
                let a = self.anim(&path)?;
                let kind = if a.dirs.len() >= 4 { Kind::FourWay } else { Kind::Single };
                Some((a, kind, Self::footprint(ai.as_ref())))
            }
            "paths" => Some((self.anim(&format!("paths/{t}/idle/idle.ani"))?, Kind::Path, 1.0)),
            "animals" | "guests" | "staff" => {
                let g = o.group.as_str();
                let ai = self.ai(&format!("{g}/{t}.ai"));
                let idle = Self::anim_name(ai.as_ref(), sub, "idle");
                // Females usually share the male graphics (no `f` folder).
                let bases: Vec<String> = match sub {
                    Some(s) => vec![format!("{g}/{t}/{s}"), format!("{g}/{t}/m")],
                    None => vec![format!("{g}/{t}")],
                };
                // Preferred idle names, then anything the .ai lists.
                // Animations made only of mask frames (swimmers' `ssub*`
                // shadows) are skipped.
                let mut names = vec![idle.clone(), "stand".into(), "idle".into(), "walk".into()];
                if let Some(ai) = &ai {
                    for sec in ai.sections.iter().filter(|s| s.name.to_ascii_lowercase().ends_with("animations")) {
                        names.extend(sec.entries.iter().filter_map(|e| e.value.clone()));
                    }
                }
                let a = bases.iter().find_map(|base| {
                    names
                        .iter()
                        .find_map(|n| self.anim(&format!("{base}/{n}/{n}.ani")).filter(|a| !anim_is_empty(a)))
                })?;
                let kind = if a.dirs.len() >= 5 { Kind::FiveWay } else { Kind::Single };
                Some((a, kind, 1.0))
            }
            _ => None,
        }
    }
}

/// No visible pixels in any frame (e.g. animations made of mask frames).
pub fn anim_is_empty(a: &Anim) -> bool {
    a.dirs.iter().all(|(f, _)| f.iter().all(|fr| fr.rect.w <= 1 && fr.rect.h <= 1))
}

pub fn load(vfs: &Vfs, map: &ZooMap) -> ObjectSet {
    let scenery_ai = vfs
        .paths_with_prefix("scenery/")
        .into_iter()
        .filter_map(|p| {
            let stem = p.strip_suffix(".ai")?;
            Some((stem.rsplit('/').next()?.to_string(), p.to_string()))
        })
        .collect();
    let mut l = Loader { vfs, atlas: AtlasBuilder::new(), anims: HashMap::new(), palettes: HashMap::new(), scenery_ai };

    // Path connectivity: tiles that hold a path.
    let path_tiles: HashSet<(i32, i32)> = map
        .objects
        .iter()
        .filter(|o| o.group == "paths")
        .filter_map(|o| o.placement.as_ref().map(|p| (p.x.div_euclid(64), p.y.div_euclid(64))))
        .collect();

    let mut drawables = Vec::new();
    let mut missing = HashSet::new();
    for o in &map.objects {
        let Some(p) = &o.placement else { continue };
        if o.group == "tankwall" {
            continue; // drawn by the water module
        }
        let Some((anim, kind, footprint)) = l.resolve(o) else {
            missing.insert(format!("{}/{}", o.group, o.type_name));
            continue;
        };
        let (tx, ty) = (p.x.div_euclid(64), p.y.div_euclid(64));
        let path_links = [(tx, ty - 1), (tx + 1, ty), (tx, ty + 1), (tx - 1, ty)].map(|t| path_tiles.contains(&t));
        let path_corners =
            map.tile(tx.max(0) as u32, ty.max(0) as u32).map(|t| t.corners()).unwrap_or([0; 4]);
        let tier = if kind == Kind::Path { 0 } else { 1 };
        let swim = l.swim_anims(o);
        drawables.push(Drawable {
            pos: Vec3::new(p.x as f32 / 64.0, p.y as f32 / 64.0, p.z as f32 / 16.0),
            anim,
            kind,
            facing: p.facing,
            phase: p.id.wrapping_mul(2654435761) % 100_000,
            depth_bias: 0.75 + footprint * 0.5,
            tier,
            path_links,
            path_corners,
            translucent: false,
            schedule: None,
            plane: Plane::Billboard,
            tint: None,
            sort_bias: 0.0,
            swim,
        });
    }
    crate::water::add(&mut l, map, &mut drawables);
    let mut missing: Vec<String> = missing.into_iter().collect();
    missing.sort();
    ObjectSet { drawables, atlas: l.atlas, missing }
}

/// Path variant (index into the 20 `.ani` directions) for the current rotation.
fn path_variant(d: &Drawable, rotation: u32) -> usize {
    let r = rotation as usize;
    // Screen side s (UL, UR, LR, LL) shows world side (s - r) mod 4.
    let side = |s: usize| d.path_links[(s + 4 - r % 4) % 4];
    let c = d.path_corners; // NW, NE, SE, SW
    if c.iter().any(|&h| h != c[0]) {
        // Screen corners (left, top, right, bottom) are world corners shifted by rotation.
        let sc = |s: usize| c[(s + 4 - r % 4) % 4];
        let (left, top, right, bottom) = (sc(0), sc(1), sc(2), sc(3));
        return if left + top > right + bottom {
            if top + right >= left + bottom { 1 } else { 0 }
        } else if top + right >= left + bottom {
            2
        } else {
            3
        };
    }
    4 + 8 * side(0) as usize + 4 * side(1) as usize + 2 * side(2) as usize + side(3) as usize
}

/// Build sprite instances for the current time and rotation, sorted back to front.
pub fn instances(set: &ObjectSet, rotation: u32, map_w: f32, map_h: f32, time_ms: u32) -> Vec<SpriteInstance> {
    let rot = |u: f32, v: f32| match rotation & 3 {
        1 => (map_h - v, u),
        2 => (map_w - u, map_h - v),
        3 => (v, map_w - u),
        _ => (u, v),
    };
    let mut keyed: Vec<(u8, f32, SpriteInstance)> = Vec::with_capacity(set.drawables.len());
    for d in &set.drawables {
        let n = d.anim.dirs.len();
        if n == 0 {
            continue;
        }
        let (dir, flip) = match d.kind {
            Kind::FourWay => {
                let side = (d.facing as usize / 2 + rotation as usize) % 4;
                (FOUR_WAY_FROM_FACING[side] % n, false)
            }
            Kind::FiveWay => {
                let f = (d.facing as usize + 2 * rotation as usize) % 8;
                match f {
                    0..=4 => (f.min(n - 1), false),
                    _ => ((8 - f).min(n - 1), true),
                }
            }
            Kind::Path => (path_variant(d, rotation).min(n - 1), false),
            Kind::Single => (0, false),
            // Rotating a quarter turn swaps which diagonal an edge runs along.
            Kind::Face { along_v } => (if along_v == (rotation % 2 == 0) { 0 } else { 1 }, false),
        };
        let (frames, ms) = &d.anim.dirs[dir.min(n - 1)];
        if frames.is_empty() {
            continue;
        }
        let t = time_ms.wrapping_add(d.phase);
        let fr = match d.schedule {
            Some(s) => {
                let t = t % s.period.max(1);
                if t >= s.active {
                    continue;
                }
                frames[(t / ms) as usize % frames.len()]
            }
            None => frames[(t / ms) as usize % frames.len()],
        };
        let (gu, gv) = rot(d.pos.x, d.pos.y);
        let key = (gv - gu) + d.pos.z * 0.5 + d.sort_bias;
        keyed.push((
            d.tier,
            key,
            SpriteInstance {
                world: d.pos.to_array(),
                layer: fr.rect.layer,
                offset: fr.offset,
                size: [fr.rect.w as f32, fr.rect.h as f32],
                uv: fr.rect.uv(),
                depth_bias: d.depth_bias,
                flags: if flip { FLAG_FLIP } else { 0 }
                    | if d.translucent { FLAG_TRANSLUCENT } else { 0 }
                    | d.plane.flags(),
                tint: d.tint.map(|[r, g, b]| [r, g, b, 0.5]).unwrap_or([0.0; 4]),
            },
        ));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    keyed.into_iter().map(|k| k.2).collect()
}
