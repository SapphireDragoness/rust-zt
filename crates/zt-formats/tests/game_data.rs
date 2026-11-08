//! Tests against the real game data. Set RUST_ZT_DATA to the game folder;
//! without it these tests pass trivially.

use zt_formats::{Ani, Ini, Palette, Sprite, strings::Strings};
use zt_vfs::{GameLayout, Vfs};

fn vfs() -> Option<(GameLayout, Vfs)> {
    let dir = std::env::var_os("RUST_ZT_DATA")?;
    let layout = GameLayout::detect(dir.as_ref()).expect("RUST_ZT_DATA has no game data");
    let vfs = Vfs::from_layout(&layout).expect("mount failed");
    Some((layout, vfs))
}

#[test]
fn zebra_walk_round_trips() {
    let Some((_, vfs)) = vfs() else { return };
    let ani = Ani::parse(&vfs.read_text("animals/zebra/m/walk/walk.ani").unwrap());
    assert_eq!(ani.animations.len(), 5);
    for i in 0..5 {
        let bytes = vfs.read(&ani.sprite_path(i).unwrap()).unwrap();
        let s = Sprite::parse(&bytes).unwrap();
        assert_eq!(s.to_bytes(), bytes);
        let pal = Palette::parse(&vfs.read(&s.palette_path).unwrap()).unwrap();
        assert_eq!(pal.to_bytes(), vfs.read(&s.palette_path).unwrap());
    }
}

#[test]
fn marine_mask_frames_parse() {
    let Some((_, vfs)) = vfs() else { return };
    let s = Sprite::parse(&vfs.read("animals/dolphin/y/ssubeat/se").unwrap()).unwrap();
    assert!(s.frames.iter().any(|f| f.mask));
    assert_eq!(s.to_bytes(), vfs.read("animals/dolphin/y/ssubeat/se").unwrap());
}

#[test]
fn configs_and_strings() {
    let Some((layout, vfs)) = vfs() else { return };
    let ai = Ini::parse(&vfs.read_text("animals.ai").unwrap());
    assert_eq!(ai.get("Global", "Class"), Some("ztunit"));
    assert!(ai.get_int("Characteristics/Integers", "cHungerCheck").is_some());
    let mut strings = Strings::default();
    for dll in layout.string_dlls() {
        strings.add_dll(&std::fs::read(dll).unwrap()).unwrap();
    }
    assert_eq!(strings.get(3270), Some("Arial Bold"));
}
