//! App state, Wayland protocol handlers and input handling for the winsplit
//! nested compositor.

use std::{
    os::unix::io::OwnedFd,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use smithay::{
    backend::{
        input::{
            Axis, ButtonState, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        },
        renderer::{
            element::{
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind,
            },
            gles::GlesRenderer,
            utils::{draw_render_elements, on_commit_buffer_handler},
            Color32F, Frame, Renderer,
        },
        winit::{WinitKeyboardInputEvent, WinitMouseInputEvent, WinitMouseWheelEvent},
    },
    delegate_compositor, delegate_data_device, delegate_seat, delegate_shm,
    delegate_xdg_decoration, delegate_xdg_shell,
    input::{
        keyboard::{FilterResult, KeyboardHandle},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle},
        Seat, SeatHandler, SeatState,
    },
    output::{Mode, Output},
    reexports::wayland_protocols::xdg::{
        decoration::zv1::server::zxdg_toplevel_decoration_v1, shell::server::xdg_toplevel,
    },
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        Client,
    },
    utils::{Physical, Rectangle, Serial, Size, Transform, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, BufferAssignment, CompositorClientState, CompositorHandler,
            CompositorState, SurfaceAttributes, TraversalAction,
        },
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::xdg::{
            decoration::{XdgDecorationHandler, XdgDecorationState},
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

/// Maximum number of guest apps winsplit will host.
pub const MAX_GUESTS: usize = 2;

/// Pointer distance (px) to the divider center that still counts as a hit.
const DIVIDER_HIT: f64 = 6.0;

/// Rendered width (px) of the divider bar.
const DIVIDER_W: i32 = 3;

/// Linux BTN_LEFT.
const BTN_LEFT: u32 = 0x110;

/// A guest app: its toplevel surface inside the container.
#[derive(Clone)]
pub struct Guest {
    pub surface: ToplevelSurface,
}

/// All mutable compositor state.
pub struct App {
    // Wayland protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    #[allow(dead_code)]
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<App>,
    pub data_device_state: DataDeviceState,
    // `seat` and `xdg_decoration_state` are kept alive on App: they own the
    // global registration and per-client dispatch state.
    #[allow(dead_code)]
    pub seat: Seat<App>,

    // winsplit state
    /// Guests in slot order: [left, right].
    pub guests: [Option<Guest>; MAX_GUESTS],
    /// Split ratio: fraction of the width given to the left guest (0.15–0.85).
    pub ratio: f64,
    /// Some while the divider is being dragged.
    pub divider_grab: Option<f64>,
    /// Last known pointer position in output pixels, top-left origin.
    pub pos: Option<(f64, f64)>,
    /// Current output (container window) size, physical px.
    pub size: Size<i32, Physical>,
    /// Host scale factor.
    pub scale: f64,
    /// The single output guests see.
    pub output: Output,
    /// Set when the host window was closed.
    pub closing: bool,
    /// True once at least one guest has connected (guards the "all gone" exit).
    pub ever_had_guest: bool,
}

impl App {
    /// Physical width of the left guest's area (px).
    pub fn left_width(&self) -> i32 {
        (self.size.w as f64 * self.ratio).round() as i32
    }

    /// Physical width of a guest's area. A lone guest takes the full width.
    pub fn guest_physical_width(&self, slot: usize) -> i32 {
        if self.guests[slot ^ 1].is_none() {
            self.size.w
        } else if slot == 0 {
            self.left_width()
        } else {
            self.size.w - self.left_width()
        }
    }

    /// Which guest is under (x, y), with the position relative to that
    /// guest's origin. `None` inside the divider hit zone or over an empty slot.
    pub fn hit_test(&self, x: f64, y: f64) -> Option<(&Guest, (f64, f64))> {
        let left_w = self.left_width() as f64;
        if (x - left_w).abs() <= DIVIDER_HIT {
            return None;
        }
        if x < left_w {
            self.guests[0].as_ref().map(|g| (g, (x, y)))
        } else {
            self.guests[1].as_ref().map(|g| (g, (x - left_w, y)))
        }
    }

    /// Tell a guest its current assigned size (in logical px) and strip its
    /// decorations. No-op when the slot is empty.
    pub fn configure_guest(&mut self, slot: usize) {
        let Some(guest) = &self.guests[slot] else {
            return;
        };
        let scale = self.scale.max(1.0);
        let w = (self.guest_physical_width(slot) as f64 / scale).round() as i32;
        let h = (self.size.h as f64 / scale).round() as i32;
        log::info!(
            "configure guest {slot}: {w}x{h} logical (physical {}x{}, scale {})",
            self.guest_physical_width(slot),
            self.size.h,
            scale
        );
        guest.surface.with_pending_state(|state| {
            state.size = Some((w, h).into());
            state.states.set(xdg_toplevel::State::Activated);
            // Ask guests to drop their CSD titlebars (apps implementing the
            // xdg-decoration protocol).
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        guest.surface.send_pending_configure();
    }

    /// Reconfigure every guest to the current layout.
    pub fn reconfigure_all(&mut self) {
        for slot in 0..MAX_GUESTS {
            self.configure_guest(slot);
        }
    }

    /// The container window was resized by the host.
    pub fn on_resized(&mut self, size: Size<i32, Physical>) {
        log::info!("container resized: {}x{} physical", size.w, size.h);
        self.size = size;
        self.output.change_current_state(
            Some(Mode {
                size,
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        self.reconfigure_all();
    }

    /// Drop dead guests (client gone or surface destroyed).
    pub fn sync_guests(&mut self) {
        let mut changed = false;
        for (slot, guest) in self.guests.iter_mut().enumerate() {
            if let Some(g) = guest.take() {
                if g.surface.alive() {
                    *guest = Some(g);
                } else {
                    log::info!("guest {slot} is gone");
                    changed = true;
                }
            }
        }
        if changed {
            // A guest died: the survivor (if any) re-takes its area.
            self.reconfigure_all();
        }
    }

    /// Number of live guest slots.
    pub fn guest_count(&self) -> usize {
        self.guests.iter().filter(|g| g.is_some()).count()
    }

    /// Render both guests + the divider into the window backbuffer and send
    /// frame callbacks. The caller submits the backbuffer afterwards.
    pub fn render_frame(
        &self,
        backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (renderer, mut framebuffer) = backend.bind()?;
        let size = self.size;
        let damage = Rectangle::from_size(size);
        let left_w = self.left_width();
        let scale = self.scale;

        let mut elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
        for (slot, guest) in self
            .guests
            .iter()
            .enumerate()
            .filter_map(|(slot, g)| g.clone().map(|g| (slot, g)))
        {
            let offset = if slot == 0 { 0 } else { left_w };
            elements.extend(render_elements_from_surface_tree(
                renderer,
                guest.surface.wl_surface(),
                (offset, 0),
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }

        let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
        frame.clear(Color32F::new(0.06, 0.06, 0.08, 1.0), &[damage])?;
        // The scale must match the output scale advertised to guests: it is
        // what scales a guest's logical surface into its physical slot.
        draw_render_elements(&mut frame, scale, &elements, &[damage])?;

        // Divider bar between the two guests (only when both are present).
        if self.guest_count() == MAX_GUESTS {
            frame.clear(
                Color32F::new(0.45, 0.45, 0.5, 1.0),
                &[Rectangle::new(
                    (left_w - DIVIDER_W / 2, 0).into(),
                    (DIVIDER_W, size.h).into(),
                )],
            )?;
        }
        let _ = frame.finish()?;

        let time = now_ms();
        for guest in self.guests.iter().flatten() {
            send_frames_surface_tree(guest.surface.wl_surface(), time);
        }
        Ok(())
    }
}

/// Pointer moved: update the divider grab, or forward focus + motion to the
/// guest under the cursor.
pub fn handle_motion(
    app: &mut App,
    keyboard: &KeyboardHandle<App>,
    pointer: &PointerHandle<App>,
    time: u32,
) {
    let Some((x, y)) = app.pos else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();

    if app.divider_grab.is_some() {
        app.ratio = (x / app.size.w.max(1) as f64).clamp(0.15, 0.85);
        app.reconfigure_all();
        return;
    }

    let event = MotionEvent {
        location: (x, y).into(),
        serial,
        time,
    };
    // Clone the surface out first: hit_test borrows app immutably.
    let (surf, focus_pos) = match app.hit_test(x, y) {
        Some((guest, pos)) => (Some(guest.surface.wl_surface().clone()), Some(pos)),
        None => (None, None),
    };
    match (surf, focus_pos) {
        (Some(surf), Some((fx, fy))) => {
            keyboard.set_focus(app, Some(surf.clone()), serial);
            pointer.motion(app, Some((surf, (fx, fy).into())), &event);
        }
        _ => {
            keyboard.set_focus(app, None, serial);
            pointer.motion(app, None, &event);
        }
    }
}

/// Pointer button: start/end a divider drag, or forward to the focused guest.
pub fn handle_button(
    app: &mut App,
    keyboard: &KeyboardHandle<App>,
    pointer: &PointerHandle<App>,
    time: u32,
    event: &WinitMouseInputEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    let code = event.button_code();
    let state = event.state();

    if app.divider_grab.is_some() {
        if code == BTN_LEFT && state == ButtonState::Released {
            app.divider_grab = None;
            // Re-establish guest focus at the current pointer position.
            handle_motion(app, keyboard, pointer, time);
        }
        return;
    }

    if code == BTN_LEFT && state == ButtonState::Pressed {
        if let Some((x, _)) = app.pos {
            let left_w = app.left_width() as f64;
            if (x - left_w).abs() <= DIVIDER_HIT {
                app.divider_grab = Some(app.ratio);
                return;
            }
        }
    }

    pointer.button(
        app,
        &ButtonEvent {
            serial,
            time,
            button: code,
            state,
        },
    );
}

/// Scroll: forward to the focused guest.
pub fn handle_axis(
    app: &mut App,
    pointer: &PointerHandle<App>,
    time: u32,
    event: &WinitMouseWheelEvent,
) {
    let mut frame = AxisFrame::new(time).source(event.source());
    for axis in [Axis::Horizontal, Axis::Vertical] {
        if let Some(v) = event.amount(axis) {
            frame = frame.value(axis, v);
        }
        if let Some(v) = event.amount_v120(axis) {
            frame = frame.v120(axis, v as i32);
        }
    }
    pointer.axis(app, frame);
    pointer.frame(app);
}

/// Key: forward to the focused guest.
pub fn handle_key(
    app: &mut App,
    keyboard: &KeyboardHandle<App>,
    _time: u32,
    event: &WinitKeyboardInputEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    keyboard.input::<(), _>(
        app,
        event.key_code(),
        event.state(),
        serial,
        event.count(),
        |_, _, _| FilterResult::Forward,
    );
}

/// Release keys the host never released (the classic Alt+Tab case: the
/// release is delivered to the window that gains focus, so this one keeps a
/// phantom modifier). Forwards a synthetic release to the focused guest,
/// which keeps the guest's modifier state in sync.
pub fn release_stuck_keys(app: &mut App, keyboard: &KeyboardHandle<App>) {
    for key in keyboard.pressed_keys() {
        log::info!("releasing stuck key {key:?}");
        keyboard.input::<(), _>(
            app,
            key,
            KeyState::Released,
            SERIAL_COUNTER.next_serial(),
            now_ms(),
            |_, _, _| FilterResult::Forward,
        );
    }
}

/// Milliseconds since an arbitrary epoch (good enough for frame callbacks).
fn now_ms() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis() as u32
}

/// Send frame callbacks through the whole surface tree.
///
/// Without this, guests that wait for a frame callback after a configure
/// will never paint.
pub fn send_frames_surface_tree(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

#[derive(Default)]
pub struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Per-client data stored in the wayland Display. Re-exported for main.rs.
pub type GuestClientState = ClientState;

impl BufferHandler for App {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let slot = match (self.guests[0].is_none(), self.guests[1].is_none()) {
            (true, _) => 0,
            (false, true) => 1,
            _ => {
                // A third toplevel is not hosted: it is never configured or
                // rendered, so it stays invisible (defensive path — winsplit
                // spawns exactly two guests).
                log::warn!("winsplit hosts exactly {MAX_GUESTS} guests; ignoring a third toplevel");
                return;
            }
        };
        log::info!("guest {slot} connected");
        self.ever_had_guest = true;
        self.guests[slot] = Some(Guest { surface });
        // The arrival may change the layout (e.g. a lone guest loses its
        // full width), so reconfigure everyone.
        self.reconfigure_all();
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // v1: popups (context menus, tooltips) are not mapped inside the
        // container. Guests keep working; menus simply do not appear.
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

impl XdgDecorationHandler for App {
    fn new_decoration(&mut self, _toplevel: ToplevelSurface) {
        // We always configure ServerSide; nothing to do here.
    }

    fn request_mode(
        &mut self,
        _toplevel: ToplevelSurface,
        _mode: zxdg_toplevel_decoration_v1::Mode,
    ) {
        // Policy: always strip decorations. Ignore the client preference.
    }

    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
}

impl SelectionHandler for App {
    type SelectionUserData = ();
}

impl DataDeviceHandler for App {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for App {}

impl ServerDndGrabHandler for App {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Diagnostics: log what the guest actually committed vs. what the
        // layout assigns it (layout vs. guest-size mismatches).
        let slot = self.guests.iter().position(|g| {
            g.as_ref()
                .is_some_and(|g| g.surface.wl_surface() == surface)
        });
        if let Some(slot) = slot {
            let toplevel_size = self.guests[slot]
                .as_ref()
                .unwrap()
                .surface
                .current_state()
                .size;
            with_surface_tree_downward(
                surface,
                (),
                |_, _, &()| TraversalAction::DoChildren(()),
                |_, states, &()| {
                    let mut cache = states.cached_state.get::<SurfaceAttributes>();
                    let attrs = cache.current();
                    if let Some(BufferAssignment::NewBuffer(buffer)) = &attrs.buffer {
                        let dims =
                            smithay::wayland::shm::with_buffer_contents(buffer, |_, _, data| {
                                (data.width, data.height)
                            })
                            .map(|d| format!("{d:?}"))
                            .unwrap_or_else(|_| "dma-buf".into());
                        log::info!(
                            "guest {slot} committed: buffer {dims} px @ scale {}, toplevel wants {:?}",
                            attrs.buffer_scale,
                            toplevel_size
                        );
                    }
                },
                |_, _, &()| true,
            );
        }
        on_commit_buffer_handler::<Self>(surface);
    }
}

impl ShmHandler for App {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

delegate_xdg_shell!(App);
delegate_xdg_decoration!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_data_device!(App);
