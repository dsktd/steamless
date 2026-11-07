//! A 3D Steam Controller: Valve's shell with overlays on its controls that light with the
//! input and are picked by a click, and a camera that orbits it. `yakin-view` opens it in a
//! window of its own (the `window` feature); `yakin-ui` draws it into its page.

#![allow(clippy::type_complexity)]

pub mod layout;
pub mod look;
pub mod model;
pub mod orbit;
