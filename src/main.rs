//! winsplit — nested Wayland compositor.
//!
//! Runs its own Wayland session on a single window of the host compositor
//! (via smithay's winit backend) and pins exactly two guest apps side by
//! side inside it. Guest apps believe they are in their own window; the
//! host only sees the container window.
//!
//! Architecture and invariants: see `AGENTS.md`.
//!
//! # Usage
//!
//! ```sh
//! winsplit <app1> <app2>
//! ```

mod state;

use std::{
    process::{Command, ExitCode},
    sync::Arc,
};

use smithay::{
    backend::{
        input::{AbsolutePositionEvent, Event, InputEvent},
        renderer::gles::GlesRenderer,
        winit::{self, WinitEvent},
    },
    input::{keyboard::ModifiersState, SeatState},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::wayland_server::{Display, ListeningSocket},
    reexports::winit::{
        dpi::LogicalSize, platform::pump_events::PumpStatus, window::WindowAttributes,
    },
    utils::Rectangle,
    wayland::{
        compositor::CompositorState,
        selection::data_device::DataDeviceState,
        shell::xdg::{decoration::XdgDecorationState, XdgShellState},
        shm::ShmState,
    },
};
use state::{
    handle_axis, handle_button, handle_key, handle_motion, release_stuck_keys, App,
    GuestClientState,
};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: winsplit <app1> <app2>");
        eprintln!("  pins the two apps into a single resizable split container window");
        return ExitCode::from(2);
    }
    match run(&args[1], &args[2]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("winsplit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(app1: &str, app2: &str) -> Result<(), Box<dyn std::error::Error>> {
    // --- Wayland display + protocol globals ---------------------------------
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();

    let compositor_state = CompositorState::new::<App>(&dh);
    let xdg_shell_state = XdgShellState::new::<App>(&dh);
    let xdg_decoration_state = XdgDecorationState::new::<App>(&dh);
    let shm_state = ShmState::new::<App>(&dh, vec![]);
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "winsplit");

    // --- The container window (winit backend: one window on the host) -------
    let (mut backend, mut winit_loop) = winit::init_from_attributes::<GlesRenderer>(
        WindowAttributes::default()
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_title("winsplit")
            .with_visible(true),
    )?;
    let scale = backend.scale_factor();
    let size = backend.window_size();

    // The single output the guests see.
    let output = Output::new(
        "winsplit".into(),
        PhysicalProperties {
            size: (300, 200).into(), // millimeters; dummy
            subpixel: Subpixel::Unknown,
            make: "winsplit".into(),
            model: "two-in-one".into(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size,
            refresh: 60_000,
        }),
        None,
        Some(Scale::Fractional(scale)),
        None,
    );

    let keyboard = seat.add_keyboard(Default::default(), 200, 20)?;
    // Inherit the host's locked modifiers (Num Lock / Caps Lock). The host
    // owns the physical LEDs and their state cannot be read back, so the
    // user declares it: WINSPLIT_LOCKED_MODS=numlock,capslock
    let mut locked = ModifiersState::default();
    if let Ok(spec) = std::env::var("WINSPLIT_LOCKED_MODS") {
        for part in spec.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "numlock" => locked.num_lock = true,
                "capslock" => locked.caps_lock = true,
                s if !s.is_empty() => {
                    eprintln!(
                        "winsplit: unknown WINSPLIT_LOCKED_MODS entry {s:?} (use numlock/capslock)"
                    )
                }
                _ => {}
            }
        }
    }
    if locked != ModifiersState::default() {
        log::info!("inheriting locked modifiers: {locked:?}");
        keyboard.set_modifier_state(locked);
    }
    let pointer = seat.add_pointer();

    let mut app = App {
        compositor_state,
        xdg_shell_state,
        xdg_decoration_state,
        shm_state,
        seat_state,
        data_device_state: DataDeviceState::new::<App>(&dh),
        seat,
        guests: [None, None],
        ratio: 0.5,
        divider_grab: None,
        pos: None,
        size,
        scale,
        output,
        closing: false,
        ever_had_guest: false,
    };

    // --- Listening socket + spawn the guests ---------------------------------
    let listener = ListeningSocket::bind_auto("winsplit", 0..16)?;
    let display_name = listener
        .socket_name()
        .expect("bind_auto returns a named socket");
    let mut children = Vec::new();
    for name in [app1, app2] {
        match Command::new(name)
            .env("WAYLAND_DISPLAY", display_name)
            .spawn()
        {
            Ok(child) => {
                log::info!("spawned guest: {name}");
                children.push(child);
            }
            Err(e) => eprintln!("winsplit: failed to spawn {name}: {e}"),
        }
    }
    if children.is_empty() {
        return Err("could not spawn any guest app".into());
    }

    // --- Main loop ------------------------------------------------------------
    let mut clients = Vec::new();
    loop {
        let status = winit_loop.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => app.on_resized(size),
            WinitEvent::Input(InputEvent::Keyboard { event }) => {
                let t = (event.time() / 1000) as u32;
                handle_key(&mut app, &keyboard, t, &event)
            }
            WinitEvent::Input(InputEvent::PointerMotionAbsolute { event }) => {
                app.pos = Some((
                    event.x_transformed(app.size.w),
                    event.y_transformed(app.size.h),
                ));
                handle_motion(&mut app, &keyboard, &pointer, (event.time() / 1000) as u32);
            }
            WinitEvent::Input(InputEvent::PointerButton { event }) => {
                let t = (event.time() / 1000) as u32;
                handle_button(&mut app, &keyboard, &pointer, t, &event)
            }
            WinitEvent::Input(InputEvent::PointerAxis { event }) => {
                let t = (event.time() / 1000) as u32;
                handle_axis(&mut app, &pointer, t, &event)
            }
            WinitEvent::CloseRequested => app.closing = true,
            WinitEvent::Focus(gained) => {
                if gained {
                    // The host may have eaten a modifier release while we
                    // were unfocused (e.g. Alt+Tab); clear the phantom.
                    release_stuck_keys(&mut app, &keyboard);
                }
            }
            WinitEvent::Redraw => {}
            WinitEvent::Input(_) => {}
        });

        if app.closing || matches!(status, PumpStatus::Exit(_)) {
            break;
        }

        // Drop dead guests; exit once all of them are gone.
        app.sync_guests();
        if app.ever_had_guest && app.guest_count() == 0 {
            log::info!("all guests are gone; exiting");
            break;
        }

        // Composite the frame.
        app.render_frame(&mut backend)?;

        // Service the display. All events must be dispatched and flushed to
        // the clients before swapping buffers (the swap may block).
        if let Some(stream) = listener.accept()? {
            log::debug!("new client connecting");
            clients.push(
                display
                    .handle()
                    .insert_client(stream, Arc::new(GuestClientState::default()))?,
            );
        }
        display.dispatch_clients(&mut app)?;
        display.flush_clients()?;

        backend.submit(Some(&[Rectangle::from_size(app.size)]))?;
    }

    // Kill the guest processes.
    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}
