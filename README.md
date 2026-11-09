# winsplit

Pin two apps into a **single window** on Wayland (KDE/KWin).

winsplit is a nested Wayland compositor: it runs its own Wayland session
inside one host window and tiles two guest apps side by side inside it. The
guest apps believe they are in their own window; the host compositor only
sees the container.

```
┌──────────────────────────────────────────────┐
│  winsplit (one host window; KWin chrome)     │
│  ┌──────────────────┬──────────────────┐     │
│  │  App 1           │  App 2           │     │
│  │  (50% width,     │  (50% width,     │     │
│  │   100% height)   │   100% height)   │     │
│  └──────────────────┴──────────────────┘     │
└──────────────────────────────────────────────┘
```

- Guest decorations are stripped.
- Drag the internal divider to change the split ratio.
- A guest requesting fullscreen fills its half, not the screen.
- Exactly two apps; no more.

## Usage

```sh
winsplit <app1> <app2>
```

Both apps are spawned by winsplit with `WAYLAND_DISPLAY` pointed at the
container's own Wayland socket. They connect, get pinned to their half, and
get their decorations stripped. Click a half to focus it; drag the divider to
resize. Closing the container (or all guests) quits winsplit.

### Locked keys (Num Lock / Caps Lock)

The container starts its keyboard state fresh, while the host (KWin) owns the
physical Num/Caps Lock LEDs — their state cannot be read back. If you run
winsplit with a locked key on the host, declare it:

```sh
WINSPLIT_LOCKED_MODS=numlock winsplit kate konsole
# also: capslock, or both: numlock,capslock
```

Without this, e.g. the numpad is interpreted as cursor/insert keys instead of
digits. (Re-run winsplit if you toggle a lock key on the host afterwards.)

## Development

```sh
nix develop          # or: direnv allow
cargo build
```

See `AGENTS.md` for the full agent/developer guide.
