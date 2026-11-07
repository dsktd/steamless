//! A window on the daemon: its live input, profiles and tuning, an editor of its config, and
//! the controller in 3D. Dioxus drawn by Blitz: one native process, no webview and no script;
//! every daemon call is Rust.

#![allow(clippy::type_complexity)]

pub mod app;
pub mod components;
pub mod daemon;
pub mod draft;
pub mod drawing;
pub mod editor;
pub mod layers;
pub mod live;
pub mod menu;
pub mod model;
pub mod rules;
pub mod scene;
pub mod target;
pub mod tuning;
pub mod view;
