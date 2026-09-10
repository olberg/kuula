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

## License

Apache-2.0. See [LICENSE](LICENSE).
