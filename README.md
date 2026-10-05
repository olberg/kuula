<p align="center"><img src="assets/branding/kuula-app-128.png" alt="Kuula" width="96"></p>

# Kuula

A small fantasy console. Carts are written in Lua 5.5 and run at a fixed 60 frames per second on a 320x240 or 640x480 indexed-colour screen, inside a sandbox with a cycle budget per frame. Every run is deterministic: the same cart and the same inputs produce the same frames, byte for byte.

Kuula runs on the desktop, on Windows and Linux, and on two kinds of handheld. On the Miyoo Mini and Mini Plus it is a system in OnionOS's Games list, with networking on the Plus ([handheld/miyoo/](handheld/miyoo/)). On Android it is a sideloaded app with on-screen controls, without networking so far ([handheld/android/](handheld/android/)). Each has been tried on one device.

## Try it

```
cargo build --release
target/release/kuula run examples/hello
target/release/kuula                      # boot the shell and pick a cart
target/release/kuula run alien-invaders
target/release/kuula run fenlight
target/release/kuula run kilnhollow
```

Arrow keys are the D-pad and Z and X are the A and B buttons, which is all most carts use. For a cart that uses more: C and V are X and Y, A and S are L1 and R1, Q and W are L2 and R2, Enter is Start and Right Shift is Select. Escape is the menu button. A gamepad works too, button for button; Start and Select held together are its menu button. Ctrl+1 to Ctrl+4 change the window scale.

On Windows the build is self-contained. On Linux it links the system SDL2, so install `libsdl2-dev` and `pkg-config` first.

Carts can also run without a window:

```
kuula run examples/hello --headless --frames 60 --out frames/
kuula screenshot examples/hello --frame 30 --out hello.png
kuula build mycart --out mycart.zip
```

## Writing a cart

A cart is a directory with `main.lua` in it, plus optional `cart.toml`, sprite sheets under `gfx/`, maps under `map/`, songs under `sfx/` and `music/`, banks of sound effects under `cues/` and samples under `samples/`. Define `_init`, `_update(dt)` and `_draw` and go.

- [docs/skill.md](docs/skill.md): constraints and idioms, read this first.
- [docs/api.md](docs/api.md): every call, its cycle price and its error codes. [docs/api.html](docs/api.html) is the same reference as one offline page, with search and a first-cart tutorial.
- [docs/songs.md](docs/songs.md): the song files `sfx` and `music` play, closely enough to write one from a script.
- [examples/](examples/): small carts covering sprites, tiles, saves, numerics, sound and networking.
- [alien-invaders/](alien-invaders/), [fenlight/](fenlight/) and [kilnhollow/](kilnhollow/): three complete games.

## Layout

| crate | what it is |
|---|---|
| `kuula-core` | the console: screen, palette, drawing, input, cycle meter, faults. No Lua, no SDL. |
| `kuula-lua` | the Lua 5.5 guest and the cart API bindings |
| `kuula-host-sdl` | the window, integer scaler, keyboard and gamepads, audio device |
| `kuula-host-common` | what every host shares: the 60 Hz schedule, the audio ring, the cart list, the settings file, which key is which button |
| `kuula-touch` | on-screen controls: where the picture and the buttons go, which button a finger is on, drawing both |
| `kuula-android` | the Android app: a native activity in Rust, with no SDL and no Java |
| `kuula-host-headless` | frame capture, hashes and input scripts for runs without a window |
| `kuula-net` | networking over Iroh: cart sessions, LAN discovery, the development deploy |
| `kuula-cli` | the `kuula` binary: run, shell, screenshot, build, net, deploy (to a receiver, or to an Android device over adb) |
| `kuula-mcp` | the console's tools over stdio JSON-RPC |
| `kuula-apidoc` | writes the reference tables of `docs/api.md` from the bindings and checks them |

`rom/main.lua` is the built-in shell. `crates/vendor/` holds two copied crates: the Open Module Track song engine and `mlua-sys` with a fixed string-hash seed.

## Changelog

### 0.0.4

- Android: Kuula as an app, sideloaded as an APK, with nine carts built in. On-screen controls (an 8-way D-pad, A, B and Menu) for either way up, a controller or a handheld's own buttons, and sound on the device's low-latency path. [handheld/android/](handheld/android/) has how to build, install and play it.
- Fourteen buttons. A cart whose `cart.toml` says `buttons = "all"` has X, Y, L1, R1, L2, R2, Start and Select beside the D-pad, A and B; a cart that does not say so has the six it always had, on every device. The keyboard, a gamepad and the Miyoo's own buttons give all of them, and the on-screen controls on Android show the buttons of the cart that is playing. Start and Select held together are Menu. Input scripts and the MCP tools take the new names, and `examples/buttons` shows each button with its number.
- `tline`: a line of pixels that samples the sprite sheet along a line of its own, which is a row of a floor in perspective or a column of a textured wall.
- [kilnhollow/](kilnhollow/): a third game. One level in the first person, in sectors with floors and ceilings at different heights and walls at any angle, drawn a column at a time with `tline`.
- A cart goes to an Android device with one command: `kuula deploy push <cart> --to adb` packs it, puts it on the device, starts the app on it and says whether it started or faulted. The MCP `deploy` tool takes the same target and returns the app's log and, when asked, the device's screen.
- Miyoo Mini: Kuula is a system in OnionOS's Games list, each cart with its picture, and an app under Apps. The Mini Plus runs the build with networking. A cart pushed to the handheld by a developer it has not approved is asked about on its screen.
- Cue banks: `cue(bank, name)` plays a sound effect from `cues/<bank>.omc`, a bank of a game's effects in one file, varied a little each time within the ranges the bank gives. `stop(channel)` ends what holds a channel. Alien Invaders has sound.
- Versions of a song: `music(name, fade, version)` names an arrangement, and asking for another version of the song that is playing switches to it on the same bar and beat.
- The music keeps its channels: an effect that names no channel no longer lands on one the song uses. `music_channels(n)` lets a cart give some back.
- [fenlight/](fenlight/): a second game. A lamplighter crosses a fen for five minutes while up to 300 enemies come for the lamp; its music changes version as the oil runs low.
- `spr`, `sspr` and `map` are several times faster on a 32-bit CPU, and every call's coordinates cost less there.
- `kuula shell --cart <file>` runs the shell on one cart and ends where it would have shown its list.

### 0.0.3

- Sound is Open Module Track. `sfx` and `music` play `.omc` songs, a tracker format with wave and sampler instruments, on eight channels in stereo. [docs/songs.md](docs/songs.md) describes the files, and `examples/atomic` plays a whole song.
- Development deploy: `kuula deploy push <cart> --to <ticket>` sends a cart to a machine running `kuula shell --dev-receiver`, which installs and starts it. Only developers the receiver has approved can push.
- The shell has network screens: host, join with a ticket, sessions found on the LAN (off until switched on) and relay settings. It takes gamepads and text input.
- `examples/marbles`, a two-player game, and `kuula net_sim`: two consoles over an in-memory transport with scripted input, delays and disconnects, for testing a multiplayer cart without a network.
- New system fonts (Unscii, 8x8 and 8x16), `font()` to choose one, and `print` draws UTF-8.
- `cart.toml` takes `author` and `license`.
- [docs/api.html](docs/api.html): the API reference as one offline page with search.
- A first Miyoo Mini (Plus) port in `handheld/miyoo/`, and a build without networking (`--no-default-features`).

### 0.0.2

- Peer-to-peer cart networking: `net.host`, `net.join`, `net.send`, `net.recv` and friends, one peer per session, gated by a permission in the shell settings. See the `net` section of [docs/api.md](docs/api.md) and the `examples/netbuttons` cart.
- Transcripts record network traffic too, so a networked run replays byte for byte.
- The shell keeps its settings (scale, net permission) in a file between runs. `--scale` is now optional.
- `kuula net listen` and `kuula net join`: a connectivity diagnostic.
- [docs/api.md](docs/api.md) is generated from the bindings and a check fails when it is stale, so the reference cannot drift from the code.
- Linux build: the system SDL2 is used off Windows, and Lua string hashing is seeded the same way on every platform, so conformance hashes match across OSes.

### 0.0.1

- First release: the console, the Lua 5.5 guest, the desktop and headless hosts, the shell, the MCP server, transcripts and the example carts.

## License

Apache-2.0. See [LICENSE](LICENSE).
