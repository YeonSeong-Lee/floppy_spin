//! 3D software rasterizer + HUD/menu drawing into a Vec<u32> 0x00RRGGBB framebuffer.
#![forbid(unsafe_code)]

pub mod camera;
pub mod clip;
pub mod frame;
pub mod mesh;
pub mod raster;
pub mod scene;
pub mod shade;
pub mod text;
