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
winsplit firefox terminal
```

## Development

```sh
nix develop          # or: direnv allow
cargo build
```

See `AGENTS.md` for the full agent/developer guide.
