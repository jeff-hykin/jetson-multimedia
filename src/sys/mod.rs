//! Raw bindings, one module per header layout. Every struct carries bindgen's compile-time layout assertions.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]

/// JetPack 5 and 6 (L4T R35, R36): identical layouts for every struct this crate touches.
pub mod l4t_r35_r36;
/// JetPack 7 (L4T R38): wider `NvBufSurface*`, transform rects and `jpeg_compress_struct`.
pub mod l4t_r38;
