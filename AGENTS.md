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

## Architecture (planned — update as modules land)

Module map (`src/`):

| Module | Responsibility |
|---|---|
| `main.rs` | CLI (exactly 2 app args), logging, event-loop bootstrap |
| `server.rs` | wl_display + winit backend (one window on the host) + single output |
| `shell.rs` | Custom shell handler: 2-panel tiling, decoration stripping, 2-app cap |
| `layout.rs` | Split ratio -> per-guest geometry; output-resize handling |
| `input.rs` | Pointer/keyboard forwarding to the guest under the cursor |
| `divider.rs` | Draggable divider (pointer grab) -> ratio update |
| `spawn.rs` | Launch guests with `WAYLAND_DISPLAY` pointed at winsplit's socket |
| `protocol/` | `winsplit-shell.xml`: custom protocol (decoration control, configure events) |

Invariants — do not break:

- The host sees exactly **one** toplevel (the container).
- At most **two** guest toplevels exist; a third connection is rejected.
- Every guest surface is sized to `ratio * output_w` / `(1-ratio) * output_w`
  and full output height; the divider is the only non-guest area.
- Input never crosses the divider: events are only forwarded to the guest
  whose half contains the cursor position.
- A guest requesting fullscreen gets its half, never the whole output.

## Hard-won facts (verified in this codebase; do not re-derive)

_(empty — add entries here as they are discovered)_

## Smoke test

Requires a running KDE Wayland session with at least one lightweight guest
app (e.g. `foot` or `alacritty`):

```sh
cd ~/code/winsplit
nix develop -c cargo build
./target/debug/winsplit foot foot
# Expected: one "winsplit" window on the host, two foot instances inside,
# one in each half. Drag the divider: halves resize. Close one guest: the
# pair is broken (behavior TBD). pkill -x winsplit
```

## File map

| Path | What |
|---|---|
| `src/` | the compositor (module map above) |
| `protocol/` | winsplit custom Wayland protocol XML |
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
