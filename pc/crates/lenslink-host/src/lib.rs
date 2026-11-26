//! LensLink Windows host: the code shared by the `lenslink-host` command line tool and the
//! `LensLink` tray app.

pub mod adb;
pub mod controls;
pub mod modes;
pub mod settings;
pub mod stream;
pub mod ui;
pub mod video;

/// Package name of the Android app.
pub const APP_ID: &str = "io.github.lenslink";
