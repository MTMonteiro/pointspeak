//! Núcleo do PointSpeak: lógica em Rust puro, testável sem tela, microfone ou rede.

pub mod audio;
pub mod config;
pub mod dictation;
pub mod events;
pub mod hotkey;
pub mod hud;
pub mod hud_layout;
pub mod injector;
pub mod models;
pub mod onnx_runtime;
pub mod settings;
pub mod transcriber;
