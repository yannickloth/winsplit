//! winsplit — nested Wayland compositor.
//!
//! Runs its own Wayland session on a single window of the host compositor
//! (via smithay's wayland backend) and pins exactly two guest apps side by
//! side inside it. Guest apps believe they are in their own window; the host
//! only sees the container window.
//!
//! Architecture and invariants: see `AGENTS.md`.
//!
//! # Usage
//!
//! ```sh
//! winsplit <app1> <app2>
//! ```
//!
//! winsplit spawns both apps on its own nested Wayland display and tiles
//! them side by side (left/right) in one container window.

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("winsplit {} — scaffold; compositor not implemented yet", env!("CARGO_PKG_VERSION"));
    eprintln!("winsplit: not implemented yet (scaffold)");
    std::process::exit(1);
}
