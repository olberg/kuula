//! Parts of a host that no windowing library is in: the frame schedule
//! ([`pacing`]), the audio ring with what keeps it in step with a device
//! ([`audio`]), the carts a shell lists ([`carts`]), its settings file
//! ([`settings`]), the lines a host logs about a cart it was started on
//! ([`devlog`]), which key and how much of a trigger is a button
//! ([`controls`]) and the time Start and Select are given to be pressed
//! together ([`chord`]). The SDL host re-exports the first two under its
//! own paths; the desktop binary and the Android app share the rest, so a
//! cart, a setting or a held key means the same to both.

pub mod audio;
pub mod carts;
pub mod chord;
pub mod controls;
pub mod devlog;
pub mod pacing;
pub mod settings;
