<p align="center"><img src="assets/branding/kuula-app-128.png" alt="Kuula" width="96"></p>

# Kuula

A small fantasy console. Carts are written in Lua 5.5 and run at a fixed 60 frames per second on a 320x240 or 640x480 indexed-colour screen, inside a sandbox with a cycle budget per frame. Every run is deterministic: the same cart and the same inputs produce the same frames, byte for byte.

Kuula runs on the desktop today. The design keeps the door open for handheld hardware later.

## Try it

```
cargo build --release
target/release/kuula run examples/hello
target/release/kuula                      # boot the shell and pick a cart
target/release/kuula run alien-invaders
```

Arrow keys are the D-pad, Z and X are the A and B buttons. Ctrl+1 to Ctrl+4 change the window scale.

On Windows the build is self-contained. On Linux it links the system SDL2, so install `libsdl2-dev` and `pkg-config` first.

Carts can also run without a window:

```
kuula run examples/hello --headless --frames 60 --out frames/
kuula screenshot examples/hello --frame 30 --out hello.png
kuula build mycart --out mycart.zip
```

## Writing a cart

A cart is a directory with `main.lua` in it, plus optional `cart.toml`, sprite sheets under `gfx/`, maps under `map/`, sounds under `sfx/` and `music/`. Define `_init`, `_update(dt)` and `_draw` and go.

- [docs/skill.md](docs/skill.md): constraints and idioms, read this first.
- [docs/api.md](docs/api.md): every call, its cycle price and its error codes.
- [examples/](examples/): small carts covering sprites, tiles, saves and numerics.
- [alien-invaders/](alien-invaders/): a complete game.

## Layout

| crate | what it is |
|---|---|
| `kuula-core` | the console: screen, palette, drawing, input, cycle meter, faults. No Lua, no SDL. |
| `kuula-lua` | the Lua 5.5 guest and the cart API bindings |
| `kuula-host-sdl` | desktop window, integer scaler, keyboard, 60 Hz pacing |
| `kuula-host-headless` | frame capture, hashes and input scripts for runs without a window |
| `kuula-cli` | the `kuula` binary: run, shell, screenshot, build |
| `kuula-mcp` | the console's tools over stdio JSON-RPC |

`rom/main.lua` is the built-in shell.

## Changelog

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
