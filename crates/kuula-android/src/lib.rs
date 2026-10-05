//! Kuula as an Android app: a native activity in Rust. Everything that
//! names an Android API is behind `cfg(target_os = "android")`, so the
//! workspace builds everywhere; what has no Android in it (the cart list,
//! the settings, the key tables, the lifecycle) is plain and tested on the
//! desktop.
//!
//! The activity is Android's own `NativeActivity` (`hasCode="false"` in the
//! manifest), which loads this library and calls [`android_main`]. The
//! window's buffer takes the picture and the on-screen controls
//! (`kuula_touch`), the audio is an AAudio stream fed from the ring in
//! `kuula_host_common`, and the buttons are touches, keys and a joystick's
//! hat.

pub mod carts;
pub mod controls;
pub mod keymap;
pub mod lifecycle;
pub mod resample;
pub mod saves;
pub mod settings;
pub mod shell;
pub mod view;

#[cfg(test)]
mod testdir;

#[cfg(target_os = "android")]
mod activity;
#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod audio;
#[cfg(target_os = "android")]
mod input;
#[cfg(target_os = "android")]
mod logging;
#[cfg(target_os = "android")]
mod storage;

/// The activity's entry point: named by `android.app.lib_name` in the
/// manifest, called by the glue on its own thread.
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    logging::init();
    log::info!("kuula {} starting", env!("CARGO_PKG_VERSION"));
    match app::run(&app) {
        Ok(()) => log::info!("kuula ended"),
        Err(e) => log::error!("kuula ended with an error: {e}"),
    }
}
