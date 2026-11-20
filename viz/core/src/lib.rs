//! The platform-independent half of viz. It builds for wasm32-unknown-unknown,
//! so nothing here touches a terminal or spawns a process.

pub mod kdl_codec;
pub mod model;
pub mod store;
pub mod svg;
pub mod theme;
pub mod timeline;
