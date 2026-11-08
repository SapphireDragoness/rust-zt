//! Interactive window (winit).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use glam::Vec2;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};
use zt_render::Gpu;
use zt_vfs::Vfs;

use crate::scene::Scene;

const SCROLL_PX_PER_SEC: f32 = 900.0;
const ZOOMS: [f32; 5] = [0.5, 1.0, 2.0, 3.0, 4.0];

pub fn run(vfs: Vfs, maps: Vec<String>, map: String) -> Result<()> {
    let event_loop = EventLoop::new()?;
    let mut maps = maps;
    let current = match maps.iter().position(|m| *m == map) {
        Some(i) => i,
        None => {
            maps.insert(0, map);
            0
        }
    };
    let mut app = App { vfs, maps, current, state: None, error: None };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct App {
    vfs: Vfs,
    maps: Vec<String>,
    current: usize,
    state: Option<State>,
    error: Option<anyhow::Error>,
}

struct State {
    window: Arc<Window>,
    gpu: Gpu,
    scene: Scene,
    keys: HashSet<Key>,
    cursor: Vec2,
    drag: Option<Vec2>,
    last: Instant,
    start: Instant,
}

impl App {
    fn init(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(el.create_window(
            Window::default_attributes().with_title("rust-zt").with_inner_size(winit::dpi::LogicalSize::new(1280, 800)),
        )?);
        let size = window.inner_size();
        let gpu = pollster::block_on(Gpu::new(window.clone(), size.width, size.height))?;
        let mut scene = Scene::new(&gpu, &self.vfs)?;
        scene.resize(gpu.config.width, gpu.config.height);
        scene.load_map(&gpu, &self.vfs, &self.maps[self.current])?;
        let now = Instant::now();
        self.state = Some(State {
            window,
            gpu,
            scene,
            keys: HashSet::new(),
            cursor: Vec2::ZERO,
            drag: None,
            last: now,
            start: now,
        });
        self.update_title();
        Ok(())
    }

    fn update_title(&self) {
        if let Some(s) = &self.state {
            s.window.set_title(&format!(
                "rust-zt — {} ({}/{})  zoom {}×  rotation {}",
                s.scene.map_name,
                self.current + 1,
                self.maps.len(),
                s.scene.camera.zoom,
                s.scene.camera.rotation
            ));
        }
    }

    fn switch_map(&mut self, delta: isize) {
        let n = self.maps.len() as isize;
        self.current = (self.current as isize + delta).rem_euclid(n) as usize;
        if let Some(s) = &mut self.state {
            if let Err(e) = s.scene.load_map(&s.gpu, &self.vfs, &self.maps[self.current]) {
                log::error!("{e:#}");
            }
        }
        self.update_title();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_none() {
            if let Err(e) = self.init(el) {
                self.error = Some(e);
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(s) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                s.gpu.resize(size.width, size.height);
                s.scene.resize(size.width, size.height);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let key = match &event.logical_key {
                    Key::Character(c) => Key::Character(c.to_lowercase().into()),
                    k => k.clone(),
                };
                if event.state == ElementState::Released {
                    s.keys.remove(&key);
                    return;
                }
                let first = s.keys.insert(key.clone());
                if !first || event.repeat {
                    return;
                }
                match key.as_ref() {
                    Key::Named(NamedKey::Escape) => el.exit(),
                    Key::Named(NamedKey::F11) => {
                        let fs = s.window.fullscreen().is_none().then_some(Fullscreen::Borderless(None));
                        s.window.set_fullscreen(fs);
                    }
                    Key::Character("q") => s.scene.camera.turn(-1),
                    Key::Character("e") => s.scene.camera.turn(1),
                    Key::Named(NamedKey::PageUp) => return self.switch_map(-1),
                    Key::Named(NamedKey::PageDown) => return self.switch_map(1),
                    _ => {}
                }
                self.update_title();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = Vec2::new(position.x as f32, position.y as f32);
                if let Some(prev) = s.drag {
                    s.scene.camera.center -= (p - prev) / s.scene.camera.zoom;
                    s.drag = Some(p);
                }
                s.cursor = p;
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right | MouseButton::Middle, .. } => {
                s.drag = (state == ElementState::Pressed).then_some(s.cursor);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                let cam = &mut s.scene.camera;
                let i = ZOOMS.iter().position(|z| *z >= cam.zoom).unwrap_or(1) as isize;
                let j = (i + dy.signum() as isize).clamp(0, ZOOMS.len() as isize - 1) as usize;
                // Zoom around the cursor.
                let before = (s.cursor - cam.viewport * 0.5) / cam.zoom;
                cam.zoom = ZOOMS[j];
                let after = (s.cursor - cam.viewport * 0.5) / cam.zoom;
                cam.center += before - after;
                self.update_title();
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - s.last).as_secs_f32().min(0.1);
                s.last = now;
                let mut dir = Vec2::ZERO;
                for (keys, d) in [
                    (["a", "ArrowLeft"], Vec2::NEG_X),
                    (["d", "ArrowRight"], Vec2::X),
                    (["w", "ArrowUp"], Vec2::NEG_Y),
                    (["s", "ArrowDown"], Vec2::Y),
                ] {
                    let held = s.keys.iter().any(|k| match k {
                        Key::Character(c) => c.as_str() == keys[0],
                        Key::Named(n) => format!("{n:?}") == keys[1],
                        _ => false,
                    });
                    if held {
                        dir += d;
                    }
                }
                s.scene.camera.center += dir * SCROLL_PX_PER_SEC * dt / s.scene.camera.zoom;

                if let Some(frame) = s.gpu.frame() {
                    let view = frame.texture.create_view(&Default::default());
                    let mut enc = s.gpu.device.create_command_encoder(&Default::default());
                    s.scene.render(&s.gpu, &mut enc, &view, &s.gpu.depth, (now - s.start).as_secs_f32());
                    s.gpu.queue.submit([enc.finish()]);
                    s.window.pre_present_notify();
                    s.gpu.queue.present(frame);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}
