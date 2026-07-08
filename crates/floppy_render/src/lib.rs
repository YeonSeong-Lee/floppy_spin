//! 3D software rasterizer + HUD/menu drawing into a Vec<u32> 0x00RRGGBB framebuffer.
#![forbid(unsafe_code)]

pub mod battle;
pub mod camera;
pub mod clip;
pub mod frame;
pub mod hud;
pub mod mesh;
pub mod particles;
pub mod post;
pub mod raster;
pub mod scene;
pub mod shade;
pub mod text;
pub mod vfx;
