# AGENTS.md

Guidance for LLM harnesses (OpenCode, Claude Code, ...) working in this repo.

## What this is

winsplit is a **nested Wayland compositor** written in Rust (smithay). It runs
its own Wayland session inside a **single window** on the host compositor
(KDE/KWin) and pins exactly **two** guest apps side by side inside it. Guest
apps believe they are in their own window; the host only ever sees the
container window.

Design decisions (locked in with the user — do not revisit without asking):

- The container is **one** host window. A KWin-script approach (two host
  windows tiled side by side) was explicitly rejected: the nesting must be
  inside a single window, non-negotiable.
- Language/library: **Rust + smithay 0.7** (not C + wlroots).
- Rendering: GPU (OpenGL ES via glow/EGL) with a CPU fallback (pixman) —
  pick at runtime, prefer glow.
- Chrome: the container's title bar and frame are drawn by the **host
  compositor (KWin)** — winsplit only draws the internal divider bar between
  the two halves.
- Guest apps are **spawned by winsplit** from CLI arguments
  (`winsplit firefox terminal`). Max 2 apps; a 3rd is rejected.
- Guest decorations are **stripped**.
- Guest "fullscreen" fills **its half** of the container, not the screen.
- Default split 50/50. No persistence yet.

## Commands

`cargo` is not installed on the host. Run every toolchain command through the
Nix devshell:

| Action | Command |
|---|---|
| Build | `nix develop -c cargo build` |
| Test | `nix develop -c cargo test` |
| Format | `nix develop -c cargo fmt` (gate: `cargo fmt --check`) |
| Lint | `nix develop -c cargo clippy --all-targets` (gate: zero warnings) |
| Run | requires a running Wayland session (KDE); `nix develop -c ./target/debug/winsplit <app1> <app2>` |

Definition of done for any change: build + tests green + `fmt --check` clean
+ clippy clean.

Commit style (no git identity is configured on this host):

```sh
git -c user.name=nicky -c user.email=nicky@local commit -m "..."
```

## Architecture

Module map (`src/`), as implemented:

| Module | Responsibility |
|---|---|
| `main.rs` | CLI (exactly 2 app args), logging, winit bootstrap, single output, guest spawn, main event loop (winit pump + accept + dispatch + submit) |
| `state.rs` | `App` (all protocol + layout state), layout math, hit-test, configure, input free fns (`handle_motion`/`handle_button`/`handle_axis`/`handle_key`/`release_stuck_keys`), all smithay handler impls, `render_frame` |

Planned but not split out yet: `protocol/` custom protocol (decoration
control), persistence of the split ratio.

Invariants — do not break:

- The host sees exactly **one** toplevel (the container).
- At most **two** guest toplevels exist; a third connection is rejected.
- Every guest surface is sized to `ratio * output_w` / `(1-ratio) * output_w`
  and full output height; the divider is the only non-guest area.
- Input never crosses the divider: events are only forwarded to the guest
  whose half contains the cursor position.
- A guest requesting fullscreen gets its half, never the whole output.

## Hard-won facts (verified in this codebase; do not re-derive)

### Nix devshell runtime libs
- `nix develop` puts dev packages' **bin** dirs on `PATH` but NOT their
  runtime **lib** dirs on `LD_LIBRARY_PATH`. The cargo-built binary (no Nix
  RPATH patching) then fails at runtime: `NoWaylandLib` (dlopen of
  `libwayland-client`) or a panic loading `libEGL.so.1`.
  Fix lives in `flake.nix` devShell:
  `LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [wayland libxkbcommon libglvnd mesa pixman]`.
- `libEGL.so.1` comes from **`libglvnd`** (the loader), not `mesa` (the
  ICD/driver). Both are needed for the glow/EGL path.

### smithay 0.7 API gotchas (winit nested backend)
- `winit::init_from_attributes::<GlesRenderer>(WindowAttributes)` →
  `(WinitGraphicsBackend, WinitEventLoop)`. Per frame:
  `backend.bind()` → `renderer.render(&mut framebuffer, size, Transform::Flipped180)`
  → draw → `frame.finish()` → `display.dispatch_clients()` + `flush_clients()`
  → `backend.submit(Some(&[damage]))`. Dispatch+flush BEFORE submit.
- **Render scale**: `draw_render_elements(&mut frame, scale, &elements,
  &damage)` and `render_elements_from_surface_tree(..., scale, ...)` must be
  given the **output scale advertised to guests** (here `Scale::Fractional`).
  Passing `1.0` (as smithay's `examples/minimal.rs` does — it advertises no
  output) renders guest surfaces 1:1, i.e. smaller than their slot on a
  fractional-scale host. This was the "guests don't fill their half" bug.
- No `Seat::next_serial()`: use `smithay::utils::SERIAL_COUNTER.next_serial()`.
- `KeyboardHandle`/`PointerHandle` are **not `Clone`** — keep them local in
  `main.rs`; the input handlers are free fns taking `&mut App` + the handles.
  (Storing the handles on `App` and calling `self.pointer.motion(self, ..)`
  fails with E0502 self-borrows.)
- `XdgDecorationState` is in `shell::xdg::decoration` (not re-exported at
  `shell::xdg`), needs `delegate_xdg_decoration!` imported from the crate
  root plus an `XdgDecorationHandler` impl, else `XdgDecorationState::new`
  errors with missing `GlobalDispatch`/`Dispatch`.
- `wl_surface` has **no server-side `destroy()`** (it's a client→server
  request). A 3rd toplevel is simply never configured/rendered → invisible.
- `WlSurface` is compared by object id (`a == b`), so finding "which guest
  slot owns this committed surface" is a simple `position(|g| g.wl_surface() == surface)`.
- `CachedState::current()` takes `&mut self` and the temporary from
  `.get::<T>()` must be bound to a `let` (else E0716).
- `with_buffer_contents(buffer, |ptr, len, data| ..)` — 3-arg closure,
  `data.width/height` (0.7 signature, not the older `&ShmBufferContents`).
- `frame.finish()` returns a must-use `Option<SyncPoint>`: `let _ = frame.finish()?`.
- `WinitEvent` matches need a wildcard arm (`WinitEvent::Input(_) => {}`) —
  the `Input` enum has more variants (touch, device add/remove) than you use.
- `seat` from `seat_state.new_wl_seat(..)` must be `let mut` (add_keyboard
  mutates registration).

### Input semantics (nested under KWin)
- Keymap: `XkbConfig::default()` compiles from the `XKB_DEFAULT_*` env, so a
  Belgian layout is picked up automatically when launched from the user
  session.
- **Stuck modifier (the `^[` bug)**: when focus leaves mid-modifier (the
  classic Alt+Tab), KWin delivers the release to the *new* focused window, so
  winsplit keeps a phantom modifier. Guests then see e.g. Alt held → every
  char arrives ESC-prefixed. Fix: on `WinitEvent::Focus(true)`, synthesize
  releases for `keyboard.pressed_keys()` (`release_stuck_keys`).
- **Locked keys (Num/Caps Lock)**: the nested xkb state starts fresh; the
  host owns the physical LEDs and `/dev/input` is not readable by the user
  (no `input` group), so the state cannot be read back. The user declares it
  via `WINSPLIT_LOCKED_MODS=numlock,capslock` → applied with
  `keyboard.set_modifier_state` before guests connect. Without it the numpad
  yields cursor/insert keys instead of digits.
- `KeyboardInnerHandle::input` calls the `#[instrument]`ed `send_keymap` on
  **every** forwarded key (info level); it no-ops when the keymap is
  unchanged, but the log line is per-keystroke. Use `RUST_LOG` to tune; don't
  "fix" the spam by caching — it's a smithay internal.

### Runtime observations
- Guest connect latency varies a lot on this machine: konsole connects in a
  couple seconds, kate can take ~3 min (heavy Qt startup). Configure happens
  at `new_toplevel`; a late guest is configured with the *current* layout.
- A lone guest is configured full-width; on the 2nd guest's arrival both are
  reconfigured to their halves (see `reconfigure_all`).

## Smoke test

Requires a running KDE Wayland session (KWin) with two guest apps installed
(`kate` and `konsole` are known-good here):

```sh
cd ~/code/winsplit
nix develop -c cargo build
# add WINSPLIT_LOCKED_MODS=numlock if the host has Num Lock on
WINSPLIT_LOCKED_MODS=numlock nix develop -c ./target/debug/winsplit kate konsole
# Expected: one "winsplit" window on the host; kate in the left half, konsole
# in the right, both filling their half; a thin divider between. Type in
# konsole: characters + numpad arrive without ^[ prefixes. Drag the divider:
# halves reconfigure live. pkill -x winsplit to stop.
```

## File map

| Path | What |
|---|---|
| `src/` | the compositor (module map above) |
| `README.md` | user-facing documentation |
| `flake.nix`, `flake.lock` | package (rustPlatform), devShell |
| `.envrc` | nix-direnv (2.3.0) |
| `opencode.jsonc` | agent plugin/agent config (see below) |

## Agent plugins (opencode.jsonc)

`opencode.jsonc` carries the standard audit-agent pins (ten general-skills
audit agents pinned to `deepseek/deepseek-v4-pro` because their frontmatter
model aliases are dropped at registration and would fall back to the session
default local model) plus the `agentmem` plugin and its prompt injection.
Same contract as `../modelmux/opencode.jsonc`: the `agent` block replaces
plugin registration, so each pin carries the full agent (`prompt` via
`{file:...}`, `mode`, permission block).

## Conventions

- Rust 2021, stable. Keep clippy + rustfmt clean on every change.
- New dependencies need justification before adding. Current stack: smithay
  0.7 (features: `backend_winit`, `wayland_frontend`, `renderer_glow`,
  `renderer_pixman`), log + env_logger.
- smithay's event loop is the single loop; do not introduce tokio or other
  async runtimes without justification.
- Docs stay in sync: `README.md` on user-visible change, the invariants list
  above on behavior change.
