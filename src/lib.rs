//! Sonido: reproductor de música nativo para Linux basado en addons
//! (protocolo de Eclipse Music).
//!
//! Módulos:
//! * [`artwork`] – portadas de relleno por ISRC (API pública de iTunes)
//! * [`models`]  – tipos del protocolo (manifest, search, stream, catalog…)
//! * [`addon`]   – cliente HTTP de un addon
//! * [`net`]     – registro de addons instalados + fan-out de peticiones
//! * [`resolve`] – resolución de streams, caché y cadena de fallback
//! * [`player`]  – cola, gapless y backends (libmpv / rodio)
//! * [`images`]  – loader de portadas con caché en memoria y disco
//! * [`ui`]      – interfaz egui
//!
pub mod addon;
pub mod artwork;
pub mod config;
pub mod headless;
pub mod http;
pub mod images;
pub mod models;
pub mod net;
pub mod player;
pub mod resolve;
pub mod ui;
pub mod util;
