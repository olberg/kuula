//! The Kuula fantasy console, headless.
//!
//! This crate owns the buffers and the screen, the palette, the
//! rasteriser, the system font, the manifest and asset decoders, the
//! guest lifecycle and the cart source abstraction. It has no platform
//! code: no SDL, no Lua. A host drives it through [`Console::step`], and a
//! guest implementation (see `kuula-lua`) plugs in through the [`Guest`]
//! trait.

pub mod assets;
pub mod audio;
pub mod blit;
pub mod buf;
pub mod codec;
pub mod console;
pub mod draw;
pub mod error_screen;
pub mod fault;
pub mod font;
pub mod input;
pub mod manifest;
pub mod meter;
pub mod net;
pub mod palette;
pub mod pen;
pub mod raster;
pub mod resources;
pub mod save;
pub mod shell;
pub mod snapshot;
pub mod source;
pub mod transcript;
pub mod zipsource;

pub use buf::{Buf, BufKind, MapInfo, Rect};
pub use console::{Console, ConsoleState, FrameOutput, Guest, Preload};
pub use draw::DrawState;
pub use fault::Fault;
pub use input::FrameInput;
pub use manifest::{Manifest, ScreenMode, Service};
pub use meter::{BudgetExceeded, Category, FrameProfile, Meter, CATEGORY_COUNT};
pub use palette::{Palette, PaletteError, PALETTE_SIZE, SYSTEM_COLOURS};
pub use pen::{Colour, Fillp};
pub use resources::{BufId, GfxError};
pub use save::{FileStore, MemoryStore, SaveError, SaveStore, WriteThroughStore};
pub use snapshot::{Snapshot, SnapshotLimits};
pub use source::{CartSource, DirSource, SourceError};
pub use transcript::{Recorder, RecordingGuest, ReplayGuest, SharedRecorder, Transcript};
pub use zipsource::ZipSource;

/// Simulation rate in frames per second. Fixed at 60.
pub const FRAME_RATE: u32 = 60;

/// Seconds per frame, handed to `_update(dt)`.
pub const FRAME_DT: f64 = 1.0 / FRAME_RATE as f64;
