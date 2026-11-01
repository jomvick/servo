//! Keep GPUI's FreeType 0.20 imports on Servo's 0.23 FFI types and native library.
//! This facade must not declare `links` or compile a second FreeType library.
pub use freetype_sys_current::*;
