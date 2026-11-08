//! Isometric camera: projection between grid coordinates and screen pixels.

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

/// Screen pixels per elevation step.
pub const ELEV_PX: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// View centre in (rotated) screen-pixel space.
    pub center: Vec2,
    pub zoom: f32,
    /// Quarter turns, 0..=3.
    pub rotation: u32,
    pub map_size: Vec2,
    pub viewport: Vec2,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CameraUniform {
    pub center: [f32; 2],
    pub half_extent: [f32; 2],
    pub map_size: [f32; 2],
    pub rotation: u32,
    pub time: f32,
}

impl Camera {
    pub fn new(map_w: u32, map_h: u32, viewport: Vec2) -> Camera {
        let mut c = Camera {
            center: Vec2::ZERO,
            zoom: 1.0,
            rotation: 0,
            map_size: Vec2::new(map_w as f32, map_h as f32),
            viewport,
        };
        c.center = c.project(c.map_size * 0.5, 0.0);
        c
    }

    /// Rotate grid coordinates by the camera's quarter turns.
    pub fn rotate(&self, p: Vec2) -> Vec2 {
        let s = self.map_size;
        match self.rotation & 3 {
            1 => Vec2::new(s.y - p.y, p.x),
            2 => Vec2::new(s.x - p.x, s.y - p.y),
            3 => Vec2::new(p.y, s.x - p.x),
            _ => p,
        }
    }

    /// Grid position + elevation → screen pixels (before camera offset).
    pub fn project(&self, grid: Vec2, elev: f32) -> Vec2 {
        let g = self.rotate(grid);
        Vec2::new(32.0 * (g.x + g.y), 16.0 * (g.y - g.x) - ELEV_PX * elev)
    }

    /// Window pixel → grid position at elevation 0.
    pub fn unproject(&self, window_px: Vec2) -> Vec2 {
        let s = (window_px - self.viewport * 0.5) / self.zoom + self.center;
        // Invert sx = 32(u+v), sy = 16(v-u) in rotated space.
        let g = Vec2::new(s.x / 64.0 - s.y / 32.0, s.x / 64.0 + s.y / 32.0);
        let m = self.map_size;
        match self.rotation & 3 {
            1 => Vec2::new(g.y, m.y - g.x),
            2 => Vec2::new(m.x - g.x, m.y - g.y),
            3 => Vec2::new(m.x - g.y, g.x),
            _ => g,
        }
    }

    /// Rotate by `delta` quarter turns, keeping the same map point centred.
    pub fn turn(&mut self, delta: i32) {
        let focus = self.unproject(self.viewport * 0.5);
        self.rotation = (self.rotation as i32 + delta).rem_euclid(4) as u32;
        self.center = self.project(focus, 0.0);
    }

    pub fn uniform(&self, time: f32) -> CameraUniform {
        CameraUniform {
            center: self.center.to_array(),
            half_extent: (self.viewport * 0.5 / self.zoom).to_array(),
            map_size: self.map_size.to_array(),
            rotation: self.rotation,
            time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unproject_inverts_project() {
        for r in 0..4 {
            let mut c = Camera::new(75, 50, Vec2::new(800.0, 600.0));
            c.rotation = r;
            c.zoom = 2.0;
            let g = Vec2::new(12.25, 30.5);
            let s = c.project(g, 0.0);
            let w = (s - c.center) * c.zoom + c.viewport * 0.5;
            assert!((c.unproject(w) - g).length() < 1e-3, "rotation {r}");
        }
    }
}
