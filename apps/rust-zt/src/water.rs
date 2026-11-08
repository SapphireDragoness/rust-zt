//! Water: Marine Mania tanks (stacked walls, glass, gate ladder, water) and
//! transient water effects (pond waves, tank waves and ripples).
//!
//! A tank is a pit: its tiles are sunk (`tanks.cfg` `initialSink`, 4 steps)
//! and its walls are `tankwall` records on the tile edges, placed at the pit
//! floor. The exhibit record holds the wall height and water level (in steps
//! above the floor), salinity and purity. The original draws the water as
//! 50% translucent sprites: a `top` tile over each floor tile and a `front`
//! or `right` face for every step of every wall up to the water level, from
//! `water/<salt|fresh><scum>/`. The glass of glass walls is drawn the same way
//! from `water/glass<scum>/`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use glam::Vec3;
use zt_formats::Ini;
use zt_formats::zoo::{Tank, ZooMap};

use crate::objects::{Anim, Drawable, Kind, Loader, Plane, Schedule, anim_is_empty, hash};

/// Terrain types that are water (`tiletex.cfg`: 9 fresh water, 10 salt water).
const WATER_TERRAIN: [u8; 2] = [9, 10];

/// `tanks.cfg` defaults, for walls that belong to no decoded tank.
const DEFAULT_TANK: Tank =
    Tank { show: false, height: 5, water_level: 4, filling: true, salinity: 100, salinity2: 100, purity: 100 };

/// Largest tank the flood fill accepts (tiles).
const MAX_TANK_TILES: usize = 128 * 128;

/// A tile edge. `along_v`: the edge runs along v at u = `line` (tile column
/// boundary), covering tile row `idx`; otherwise it runs along u at v = `line`,
/// covering tile column `idx`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Edge {
    along_v: bool,
    line: i32,
    idx: i32,
}

impl Edge {
    /// The edge a fence-like record sits on (x, y in 1/64 tile). Facing 2 and
    /// 6 are the ±u edges, 0 and 4 the ±v edges.
    fn of_record(x: i32, y: i32, facing: u32) -> Edge {
        match facing {
            2 | 6 => Edge { along_v: true, line: (x + 32).div_euclid(64), idx: y.div_euclid(64) },
            _ => Edge { along_v: false, line: (y + 32).div_euclid(64), idx: x.div_euclid(64) },
        }
    }

    fn mid(&self) -> (f32, f32) {
        if self.along_v {
            (self.line as f32, self.idx as f32 + 0.5)
        } else {
            (self.idx as f32 + 0.5, self.line as f32)
        }
    }

    /// The tiles on the low and high side of the edge.
    fn tiles(&self) -> [(i32, i32); 2] {
        if self.along_v {
            [(self.line - 1, self.idx), (self.line, self.idx)]
        } else {
            [(self.idx, self.line - 1), (self.idx, self.line)]
        }
    }

    fn shifted(&self, d: i32) -> Edge {
        Edge { idx: self.idx + d, ..*self }
    }
}

struct Wall {
    edge: Edge,
    /// Record position (grid units, elevation steps).
    pos: Vec3,
    facing: u32,
    type_name: String,
    sub: String,
    id: u32,
}

struct TankArea {
    settings: Tank,
    tiles: HashSet<(i32, i32)>,
    /// Pit floor (elevation steps), from the walls' z.
    floor: f32,
}

/// Animation keys of the wall pieces in a tank wall's `.ai`
/// (`[f/Animations]`): bottom/mid/top rows, plus `tb*` for 1-step walls.
fn piece_key(row: &str, post_l: bool, post_r: bool) -> String {
    let col = match (post_l, post_r) {
        (true, true) => "lr",
        (true, false) => "l",
        (false, true) => "r",
        (false, false) => "m",
    };
    if row == "tb" && col == "lr" {
        return "tblidle".into(); // no single-column low piece
    }
    format!("{row}{col}idle")
}

/// Total length of one play of direction 0 (ms).
fn anim_len(a: &Anim) -> u32 {
    a.dirs.first().map(|(f, ms)| f.len() as u32 * ms).unwrap_or(1).max(1)
}

/// `water/<salt|fresh|glass><scum>` for a tank's purity (`tanks.cfg`: light
/// scum at 60, heavy at 20, extreme at 1).
fn water_dir(kind: &str, purity: u32) -> String {
    let scum = match purity {
        0..=1 => "es",
        2..=20 => "hs",
        21..=60 => "ls",
        _ => "",
    };
    format!("water/{kind}{scum}")
}

/// Waves that appear now and then: visible for one play every 1/`rate`
/// of the time on average, with a per-spot period.
fn wave_schedule(anim: &Anim, seed: u32, mean_period_ms: u32) -> Schedule {
    let active = anim_len(anim);
    let period = mean_period_ms / 2 + hash(seed ^ 0x5bd1) % mean_period_ms.max(1);
    Schedule { period: period.max(active + 500), active }
}

pub fn add(l: &mut Loader, map: &ZooMap, out: &mut Vec<Drawable>) {
    let walls: Vec<Wall> = map
        .objects
        .iter()
        .filter(|o| o.group == "tankwall")
        .filter_map(|o| {
            let p = o.placement.as_ref()?;
            Some(Wall {
                edge: Edge::of_record(p.x, p.y, p.facing),
                pos: Vec3::new(p.x as f32 / 64.0, p.y as f32 / 64.0, p.z as f32 / 16.0),
                facing: p.facing,
                type_name: o.type_name.clone(),
                sub: o.subtype.clone().unwrap_or_else(|| "f".into()),
                id: p.id,
            })
        })
        .collect();
    let edges: HashSet<Edge> = walls.iter().map(|w| w.edge).collect();

    // Tank interiors: flood fill from each tank exhibit's tile, bounded by walls.
    let mut tanks: Vec<TankArea> = Vec::new();
    for e in &map.exhibits {
        let Some(t) = e.tank else { continue };
        let Some(tiles) = flood(map, &edges, (e.x as i32, e.y as i32)) else {
            log::warn!("tank {:?}: walls don't enclose ({}, {})", e.name, e.x, e.y);
            continue;
        };
        let floor = walls
            .iter()
            .filter(|w| w.edge.tiles().iter().any(|t| tiles.contains(t)))
            .map(|w| w.pos.z)
            .fold(f32::INFINITY, f32::min);
        if !floor.is_finite() {
            continue;
        }
        tanks.push(TankArea { settings: t, tiles, floor });
    }
    let tank_of_tile: HashMap<(i32, i32), usize> =
        tanks.iter().enumerate().flat_map(|(i, t)| t.tiles.iter().map(move |&p| (p, i))).collect();

    // Walls: stacked pieces, glass, gate ladder; water faces up to the water level.
    let mut ais: HashMap<String, Option<Ini>> = HashMap::new();
    for w in &walls {
        let tank_idx = w.edge.tiles().iter().find_map(|t| tank_of_tile.get(t).copied());
        let settings = tank_idx.map(|i| tanks[i].settings).unwrap_or(DEFAULT_TANK);
        let height = settings.height.max(1);
        let base = w.pos.z;
        let ai_path = format!("fences/{}.ai", w.type_name);
        let ai = ais.entry(ai_path.clone()).or_insert_with(|| l.ai(&ai_path)).clone();
        let piece = |l: &mut Loader, key: &str| -> Option<Rc<Anim>> {
            let path = Loader::anim_name(ai.as_ref(), Some(&w.sub), key);
            if path == key { None } else { l.anim(&path) }
        };

        // Posts where the wall line ends. Facing 0/2 walls' "left" end is
        // toward +idx, facing 4/6 toward -idx (from the sprites' bounds).
        let l_dir = if w.facing < 4 { 1 } else { -1 };
        let post_l = !edges.contains(&w.edge.shifted(l_dir));
        let post_r = !edges.contains(&w.edge.shifted(-l_dir));
        let mid_anim = piece(l, &piece_key("m", false, false));
        let glass = mid_anim.as_deref().is_none_or(anim_is_empty);

        // Inward normal: which side holds the tank (water faces sit just
        // inside the wall so the painter's sort puts them on the right side).
        let (mx, my) = w.edge.mid();
        let inward = match tank_idx {
            Some(i) if tanks[i].tiles.contains(&w.edge.tiles()[1]) => 1.0,
            Some(_) => -1.0,
            None => if w.facing < 4 { 1.0 } else { -1.0 }, // walls face outward
        };
        let nudge = |d: f32| {
            if w.edge.along_v { Vec3::new(mx + d * inward, my, 0.0) } else { Vec3::new(mx, my + d * inward, 0.0) }
        };
        let kind = Kind::Face { along_v: w.edge.along_v };
        let seed = w.id;
        // Ground on the outer side (no glass is drawn below it).
        let outer = w.edge.tiles()[if inward > 0.0 { 0 } else { 1 }];
        let outer_ground = map
            .tile(outer.0.max(0) as u32, outer.1.max(0) as u32)
            .map(|t| t.corners().into_iter().min().unwrap_or(t.height) as f32)
            .unwrap_or(f32::NEG_INFINITY);
        // Faces, glass and wall pieces take their depth from the wall's
        // plane: the terrain hides what is below the outside ground, and the
        // pit's side (exactly on the edge) stays behind them.
        let plane = Plane::Edge { along_v: w.edge.along_v };
        let face = |pos: Vec3, anim: Rc<Anim>| {
            let mut d = Drawable::new(pos, anim, kind, 0, seed);
            d.translucent = true;
            d.plane = plane;
            d.depth_bias = 0.0;
            d
        };
        let piece_drawable = |pos: Vec3, anim: Rc<Anim>| {
            let mut d = Drawable::new(pos, anim, Kind::FourWay, w.facing, seed);
            d.plane = plane;
            // Some records sit exactly on the edge, level with the pit side.
            d.depth_bias = 0.01;
            d
        };

        if let Some(i) = tank_idx {
            let t = &tanks[i].settings;
            let dir = water_dir(if t.salinity >= 50 { "salt" } else { "fresh" }, t.purity);
            let faces = l
                .face_anim(&format!("{dir}/front/front.ani"), &format!("{dir}/right/right.ani"))
                .or_else(|| {
                    let k = if t.salinity >= 50 { "salt" } else { "fresh" };
                    l.face_anim(&format!("water/{k}/front/front.ani"), &format!("water/{k}/right/right.ani"))
                });
            if let Some(faces) = faces {
                for s in 0..t.water_level.min(height) {
                    out.push(face(nudge(1.0 / 32.0) + Vec3::Z * (base + s as f32), faces.clone()));
                }
            }
        }
        if glass {
            let dir = water_dir("glass", tank_idx.map(|i| tanks[i].settings.purity).unwrap_or(100));
            let panes = l
                .face_anim(&format!("{dir}/fgrey/fgrey.ani"), &format!("{dir}/lgrey/lgrey.ani"))
                .or_else(|| l.face_anim("water/glass/fgrey/fgrey.ani", "water/glass/lgrey/lgrey.ani"));
            if let Some(panes) = panes {
                // Above the water and the outside ground. In the original, the
                // strip under a near wall's rail is the same for salt and
                // fresh water: grey glass over the water seen through it.
                let water_top = tank_idx.map(|i| tanks[i].settings.water_level).unwrap_or(0);
                for s in water_top..height {
                    let z = base + s as f32;
                    if z + 1.0 > outer_ground {
                        out.push(face(nudge(0.0) + Vec3::Z * z, panes.clone()));
                    }
                }
            }
        }

        // Wall pieces: bottom, mid…, top (or one low piece).
        let rows: Vec<&str> = if height == 1 {
            vec!["tb"]
        } else {
            std::iter::once("b").chain(std::iter::repeat_n("m", height as usize - 2)).chain(std::iter::once("t")).collect()
        };
        for (s, row) in rows.iter().enumerate() {
            if let Some(a) = piece(l, &piece_key(row, post_l, post_r)) {
                if !anim_is_empty(&a) {
                    out.push(piece_drawable(Vec3::new(w.pos.x, w.pos.y, base + s as f32), a));
                }
            }
        }
        // Gates: the platform (handrails included) on the rim, a ladder down
        // the inside and one down the outside to the ground. Fitted to the
        // original: the outside ladder uses `ladrin` too (`ladrout` has dark
        // rungs, `ladrtop` doubles the handrails), hung 0.3 below the rim
        // and 0.1 tile along the wall so its rails meet the handrails; there
        // is none when the rim is at ground level.
        if w.sub == "g" {
            let top = base + height as f32;
            let at = |d: f32, z: f32| nudge(d) + Vec3::Z * z;
            if let Some(a) = ai.as_ref().and_then(|ai| ai.get("g/Animations", "ladder")).and_then(|p| l.anim(p)) {
                for s in 0..height.saturating_sub(1) {
                    out.push(piece_drawable(at(1.0 / 32.0, base + s as f32), a.clone()));
                }
                // Along the wall: the inward normal turned 90° counter-clockwise.
                let side = if w.edge.along_v { Vec3::new(0.0, inward, 0.0) } else { Vec3::new(-inward, 0.0, 0.0) };
                let ground = outer_ground.max(base);
                let mut z = top - 0.3;
                while ground < top && z > ground - 0.5 {
                    let mut d = piece_drawable(at(-1.0 / 32.0, z) - side * 0.1, a.clone());
                    // In front of the rim, the corner post and the platform.
                    d.plane = Plane::Billboard;
                    d.depth_bias = 0.3;
                    d.sort_bias = 0.15;
                    out.push(d);
                    z -= 1.0;
                }
            }
            if let Some(a) = piece(l, "platform") {
                let mut d = piece_drawable(Vec3::new(w.pos.x, w.pos.y, top), a);
                d.plane = Plane::Ground; // lies on the rim, over the water
                d.depth_bias = 0.05;
                // Drawn over the neighbouring wall and the glass.
                d.sort_bias = 0.1;
                out.push(d);
            }
        }
        // Ripples along the wall at the water line.
        if let Some(i) = tank_idx {
            if let Some(a) = l.anim("objects/edgrip/idle/idle.ani") {
                let z = tanks[i].floor + tanks[i].settings.water_level as f32;
                out.push(Drawable::new(nudge(0.25) + Vec3::Z * z, a, Kind::FourWay, w.facing, seed ^ 0x77));
            }
        }
    }

    // Animals in a tank: at the surface they show their surface swim over
    // its underwater half; below it, their animation. What is under water is
    // tinted with the tank's water colour (the original blends it 50/50).
    let mut colors: HashMap<String, Option<[f32; 3]>> = HashMap::new();
    let mut extra = Vec::new();
    for d in out.iter_mut() {
        let Some(&i) = tank_of_tile.get(&(d.pos.x.floor() as i32, d.pos.y.floor() as i32)) else { continue };
        let Some((surf, under)) = d.swim.clone() else { continue };
        let t = tanks[i].settings;
        let surface = tanks[i].floor + t.water_level as f32;
        let salt = t.salinity >= 50;
        let path = format!("{}/top/top.ani", water_dir(if salt { "salt" } else { "fresh" }, t.purity));
        let tint = *colors.entry(path.clone()).or_insert_with(|| {
            l.average_color(&path)
                .or_else(|| l.average_color(if salt { "water/salt/top/top.ani" } else { "water/fresh/top/top.ani" }))
        });
        if d.pos.z >= surface - 0.25 {
            d.anim = surf;
            if let Some(under) = under {
                // Drawn just before the surface part.
                let mut u = Drawable::new(d.pos - Vec3::Z * 0.01, under, d.kind, d.facing, 0);
                u.phase = d.phase;
                u.tint = tint;
                extra.push(u);
            }
        } else {
            d.tint = tint;
        }
    }
    out.extend(extra);

    // Water tops over tank floors, and waves on the surface.
    for (i, t) in tanks.iter().enumerate() {
        let salt = t.settings.salinity >= 50;
        let dir = water_dir(if salt { "salt" } else { "fresh" }, t.settings.purity);
        let top = l.anim(&format!("{dir}/top/top.ani")).or_else(|| {
            l.anim(if salt { "water/salt/top/top.ani" } else { "water/fresh/top/top.ani" })
        });
        let wave = l.anim(if salt { "objects/waterwv1/idle/idle.ani" } else { "objects/frshwav/idle/idle.ani" });
        let surface = t.floor + t.settings.water_level as f32;
        for &(x, y) in &t.tiles {
            let seed = (x as u32) << 16 ^ y as u32 ^ (i as u32) << 28;
            if let Some(top) = &top {
                let floor = map.tile(x as u32, y as u32).map(|t| t.height as f32).unwrap_or(t.floor);
                let mut d = Drawable::new(Vec3::new(x as f32 + 0.5, y as f32 + 0.5, floor), top.clone(), Kind::Single, 0, seed);
                d.translucent = true;
                d.plane = Plane::Ground;
                d.depth_bias = 0.02;
                out.push(d);
            }
            if t.settings.water_level > 0 && x % 2 == 0 && y % 2 == 0 {
                if let Some(wave) = &wave {
                    let mut d = Drawable::new(Vec3::new(x as f32 + 1.0, y as f32 + 1.0, surface), wave.clone(), Kind::FourWay, (hash(seed) % 4) * 2, seed);
                    d.schedule = Some(wave_schedule(wave, seed, 20_000));
                    out.push(d);
                }
            }
        }
    }

    // Ponds, rivers and sea: a wave pops up on the water now and then.
    if let Some(wave) = l.anim("objects/waterwv1/idle/idle.ani") {
        for y in (0..map.height).step_by(2) {
            for x in (0..map.width).step_by(2) {
                let Some(t) = map.tile(x, y) else { continue };
                let c = t.corners();
                if !WATER_TERRAIN.contains(&t.terrain) || c.iter().any(|&h| h != c[0]) {
                    continue;
                }
                if tank_of_tile.contains_key(&(x as i32, y as i32)) {
                    continue;
                }
                let seed = x << 16 ^ y ^ 0xa5a5_0000;
                let pos = Vec3::new(x as f32 + 0.5 + (hash(seed) % 64) as f32 / 64.0, y as f32 + 0.5 + (hash(seed ^ 1) % 64) as f32 / 64.0, c[0] as f32);
                let mut d = Drawable::new(pos, wave.clone(), Kind::FourWay, (hash(seed ^ 2) % 4) * 2, seed);
                d.schedule = Some(wave_schedule(&wave, seed, 60_000));
                d.depth_bias = 0.5;
                out.push(d);
            }
        }
    }
}

/// Tiles reachable from `start` without crossing a wall; `None` if the area
/// leaks to the map border or is implausibly large.
fn flood(map: &ZooMap, walls: &HashSet<Edge>, start: (i32, i32)) -> Option<HashSet<(i32, i32)>> {
    let (w, h) = (map.width as i32, map.height as i32);
    let inside = |(x, y): (i32, i32)| (0..w).contains(&x) && (0..h).contains(&y);
    if !inside(start) {
        return None;
    }
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some((x, y)) = queue.pop_front() {
        let steps = [
            ((x + 1, y), Edge { along_v: true, line: x + 1, idx: y }),
            ((x - 1, y), Edge { along_v: true, line: x, idx: y }),
            ((x, y + 1), Edge { along_v: false, line: y + 1, idx: x }),
            ((x, y - 1), Edge { along_v: false, line: y, idx: x }),
        ];
        for (n, e) in steps {
            if walls.contains(&e) || seen.contains(&n) {
                continue;
            }
            if !inside(n) || seen.len() >= MAX_TANK_TILES {
                return None;
            }
            seen.insert(n);
            queue.push_back(n);
        }
    }
    Some(seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_edges() {
        // From the tank test saves: (x, y) in 1/64 tile and facing.
        assert_eq!(Edge::of_record(1504, 2176, 0), Edge { along_v: false, line: 34, idx: 23 });
        assert_eq!(Edge::of_record(2111, 2208, 2), Edge { along_v: true, line: 33, idx: 34 });
        assert_eq!(Edge::of_record(1504, 3135, 4), Edge { along_v: false, line: 49, idx: 23 });
        assert_eq!(Edge::of_record(1472, 2208, 6), Edge { along_v: true, line: 23, idx: 34 });
    }

    #[test]
    fn piece_keys() {
        assert_eq!(piece_key("b", true, false), "blidle");
        assert_eq!(piece_key("m", false, false), "mmidle");
        assert_eq!(piece_key("t", true, true), "tlridle");
        assert_eq!(piece_key("tb", false, true), "tbridle");
    }

    #[test]
    fn scum_dirs() {
        assert_eq!(water_dir("salt", 100), "water/salt");
        assert_eq!(water_dir("fresh", 50), "water/freshls");
        assert_eq!(water_dir("glass", 10), "water/glasshs");
        assert_eq!(water_dir("salt", 0), "water/saltes");
    }
}
