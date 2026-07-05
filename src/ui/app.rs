use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use rmpv::Value;
use tracing::{debug, error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::ModifiersState;
use winit::window::{Icon, Theme, Window, WindowId};

use crate::config::{Config, WindowTheme};
use crate::nvim::process::{NvimEvent, NvimProcess};
use crate::nvim::redraw::{CursorStyle, RedrawEvent, decode_redraw_notification};
use crate::platform;

use super::grid::GridState;
use super::ime::ImeState;
use super::input::{WheelAccumulator, WheelDirection, key_to_nvim, nvim_modifiers};
use super::renderer::Renderer;

const MAX_RPC_EVENTS_PER_TICK: usize = 256;

#[derive(Debug, Clone, Copy)]
enum BlinkPhase {
    Waiting,
    Visible,
    Hidden,
}

#[derive(Debug)]
struct CursorBlink {
    visible: bool,
    phase: BlinkPhase,
    deadline: Option<Instant>,
    blink_on: Duration,
    blink_off: Duration,
}

impl Default for CursorBlink {
    fn default() -> Self {
        Self {
            visible: true,
            phase: BlinkPhase::Waiting,
            deadline: None,
            blink_on: Duration::ZERO,
            blink_off: Duration::ZERO,
        }
    }
}

impl CursorBlink {
    fn reset(&mut self, style: &CursorStyle, now: Instant) {
        self.visible = true;
        self.phase = BlinkPhase::Waiting;
        self.blink_on = Duration::from_millis(style.blink_on);
        self.blink_off = Duration::from_millis(style.blink_off);
        self.deadline = (style.blink_on > 0 && style.blink_off > 0)
            .then(|| now + Duration::from_millis(style.blink_wait));
    }

    fn suspend(&mut self) {
        self.visible = true;
        self.deadline = None;
    }

    fn advance(&mut self, now: Instant) -> bool {
        let Some(mut deadline) = self.deadline else {
            return false;
        };
        if now < deadline {
            return false;
        }

        let previous = self.visible;
        while now >= deadline {
            match self.phase {
                BlinkPhase::Waiting | BlinkPhase::Visible => {
                    self.visible = false;
                    self.phase = BlinkPhase::Hidden;
                    deadline += self.blink_off;
                }
                BlinkPhase::Hidden => {
                    self.visible = true;
                    self.phase = BlinkPhase::Visible;
                    deadline += self.blink_on;
                }
            }
        }
        self.deadline = Some(deadline);
        previous != self.visible
    }
}

pub struct MadoApp {
    nvim: NvimProcess,
    grid: GridState,
    ime: ImeState,
    renderer: Option<Renderer>,
    window: Option<Arc<Window>>,
    has_received_first_frame: bool,
    modifiers: ModifiersState,
    mouse_position: PhysicalPosition<f64>,
    pressed_mouse_button: Option<&'static str>,
    ime_allowed: bool,
    wheel: WheelAccumulator,
    cursor_blink: CursorBlink,
    config: Config,
}

impl MadoApp {
    pub fn new(nvim: NvimProcess, config: Config) -> Self {
        Self {
            nvim,
            grid: GridState::default(),
            ime: ImeState::default(),
            renderer: None,
            window: None,
            has_received_first_frame: false,
            modifiers: ModifiersState::empty(),
            mouse_position: PhysicalPosition::new(0.0, 0.0),
            pressed_mouse_button: None,
            ime_allowed: false,
            wheel: WheelAccumulator::default(),
            cursor_blink: CursorBlink::default(),
            config,
        }
    }

    fn process_rpc_events(&mut self, event_loop: &ActiveEventLoop) {
        for _ in 0..MAX_RPC_EVENTS_PER_TICK {
            match self.nvim.events().try_recv() {
                Ok(NvimEvent::Notification { method, params }) if method == "redraw" => {
                    match decode_redraw_notification(&params) {
                        Ok(events) => {
                            for event in &events {
                                if let RedrawEvent::ModeChange { mode, .. } = event {
                                    self.set_ime_for_mode(mode);
                                }
                            }
                            let mut flush = false;
                            for event in &events {
                                flush |= self.grid.apply(event);
                            }
                            if events.iter().any(|event| {
                                matches!(
                                    event,
                                    RedrawEvent::GridCursorGoto { .. }
                                        | RedrawEvent::ModeChange { .. }
                                        | RedrawEvent::ModeInfoSet { .. }
                                )
                            }) {
                                self.cursor_blink
                                    .reset(&self.grid.cursor_style(), Instant::now());
                            }
                            if flush {
                                if !self.has_received_first_frame {
                                    self.has_received_first_frame = true;
                                    self.update_window_title();
                                }
                                self.update_ime_cursor_area();
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                            }
                        }
                        Err(error) => warn!(%error, "invalid redraw notification"),
                    }
                }
                Ok(NvimEvent::Notification { method, .. }) => {
                    debug!(%method, "ignoring non-UI notification");
                }
                Ok(NvimEvent::ProtocolError(message)) => {
                    error!(%message, "Neovim RPC protocol error");
                }
                Ok(NvimEvent::Eof) => {
                    info!("Neovim exited");
                    event_loop.exit();
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    error!("Neovim RPC reader disconnected");
                    event_loop.exit();
                    return;
                }
            }
        }

        match self.nvim.try_wait() {
            Ok(Some(status)) => {
                info!(%status, "Neovim exited");
                event_loop.exit();
            }
            Ok(None) => {}
            Err(error) => {
                error!(%error, "failed to query Neovim process");
                event_loop.exit();
            }
        }
    }

    fn send_input(&self, input: String) {
        if let Err(error) = self
            .nvim
            .rpc()
            .notify("nvim_input", vec![Value::from(input)])
        {
            error!(%error, "failed to send input to Neovim");
        }
    }

    fn wake_cursor(&mut self) {
        self.cursor_blink
            .reset(&self.grid.cursor_style(), Instant::now());
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn resize_neovim(&self) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (width, height) = renderer.grid_dimensions();
        if let Err(error) = self.nvim.rpc().notify(
            "nvim_ui_try_resize",
            vec![Value::from(width), Value::from(height)],
        ) {
            error!(%error, "failed to resize Neovim UI");
        }
    }

    fn send_mouse(&self, button: &str, action: &str) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (row, col) = renderer.cell_at(self.mouse_position);
        let modifiers = nvim_modifiers(self.modifiers);
        if let Err(error) = self.nvim.rpc().notify(
            "nvim_input_mouse",
            vec![
                Value::from(button),
                Value::from(action),
                Value::from(modifiers),
                Value::from(1),
                Value::from(row),
                Value::from(col),
            ],
        ) {
            error!(%error, "failed to send mouse input to Neovim");
        }
    }

    fn update_ime_cursor_area(&self) {
        let (Some(renderer), Some(window)) = (&self.renderer, &self.window) else {
            return;
        };
        let (position, size) = renderer.ime_cursor_area(&self.grid, self.ime.preedit());
        window.set_ime_cursor_area(position, size);
    }

    fn set_ime_for_mode(&mut self, mode: &str) {
        let allowed = mode_allows_ime(mode);
        if self.ime_allowed == allowed {
            return;
        }
        self.ime_allowed = allowed;
        if !allowed {
            self.ime.cancel();
        }
        if let Some(window) = &self.window {
            window.set_ime_allowed(allowed);
        }
        debug!(mode, allowed, "updated IME availability for Neovim mode");
    }

    fn request_close(&mut self, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
        self.wake_cursor();
        self.send_input(close_command());
    }

    fn open_file(&self, path: &std::path::Path) {
        let command = Value::Map(vec![
            (Value::from("cmd"), Value::from("edit")),
            (
                Value::from("args"),
                Value::Array(vec![Value::from(path.to_string_lossy().into_owned())]),
            ),
            (
                Value::from("magic"),
                Value::Map(vec![
                    (Value::from("file"), Value::from(false)),
                    (Value::from("bar"), Value::from(false)),
                ]),
            ),
            (
                Value::from("mods"),
                Value::Map(vec![(Value::from("confirm"), Value::from(true))]),
            ),
        ]);
        if let Err(error) = self
            .nvim
            .rpc()
            .notify("nvim_cmd", vec![command, Value::Map(Vec::new())])
        {
            error!(path = %path.display(), %error, "failed to open file from the OS");
        } else {
            info!(path = %path.display(), "opening file from the OS");
        }
    }

    fn update_window_title(&self) {
        if let Some(window) = &self.window {
            window.set_title(window_title(self.has_received_first_frame));
        }
    }
}

impl ApplicationHandler for MadoApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(window_title(self.has_received_first_frame))
            .with_window_icon(mado_window_icon())
            .with_theme(window_theme(self.config.window.theme))
            .with_transparent(self.config.window.opacity < 1.0 || self.config.window.blur)
            .with_blur(self.config.window.blur)
            .with_inner_size(LogicalSize::new(
                self.config.window.width as f64,
                self.config.window.height as f64,
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                error!(%error, "failed to create window");
                event_loop.exit();
                return;
            }
        };
        platform::install_native_menu();
        window.set_ime_allowed(self.ime_allowed);
        let renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            event_loop,
            &self.config.font,
            self.config.window.opacity,
        )) {
            Ok(renderer) => renderer,
            Err(error) => {
                error!(%error, "failed to initialize renderer");
                event_loop.exit();
                return;
            }
        };
        self.renderer = Some(renderer);
        self.window = Some(window.clone());
        self.resize_neovim();
        self.update_ime_cursor_area();
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => self.request_close(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    let scale = self
                        .window
                        .as_ref()
                        .map_or(1.0, |window| window.scale_factor());
                    renderer.resize(size, scale);
                }
                self.resize_neovim();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let (Some(renderer), Some(window)) = (&mut self.renderer, &self.window) {
                    renderer.resize(window.inner_size(), scale_factor);
                }
                self.resize_neovim();
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = &mut self.renderer
                    && let Err(error) =
                        renderer.render(&self.grid, self.ime.preedit(), self.cursor_blink.visible)
                {
                    error!(%error, "render failed");
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if !self.ime.blocks_keyboard_input()
                    && let Some(input) = key_to_nvim(&event, self.modifiers)
                {
                    self.wake_cursor();
                    self.send_input(input);
                }
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    Ime::Enabled => {}
                    Ime::Preedit(text, cursor) => self.ime.set_preedit(text, cursor),
                    Ime::Commit(text) => {
                        if let Some(text) = self.ime.commit(text) {
                            self.send_input(text);
                        }
                    }
                    Ime::Disabled => self.ime.cancel(),
                }
                self.update_ime_cursor_area();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                self.ime.cancel();
                self.cursor_blink.suspend();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(true) => self.wake_cursor(),
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_position = position;
                if let Some(button) = self.pressed_mouse_button {
                    self.send_mouse(button, "drag");
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Some("left"),
                    MouseButton::Right => Some("right"),
                    MouseButton::Middle => Some("middle"),
                    _ => None,
                };
                if let Some(button) = button {
                    self.wake_cursor();
                    let action = if state == ElementState::Pressed {
                        self.pressed_mouse_button = Some(button);
                        "press"
                    } else {
                        self.pressed_mouse_button = None;
                        "release"
                    };
                    self.send_mouse(button, action);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (horizontal, vertical) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                    MouseScrollDelta::PixelDelta(position) => {
                        let Some(renderer) = &self.renderer else {
                            return;
                        };
                        renderer.pixel_scroll_lines(position.x, position.y)
                    }
                };
                let directions = self.wheel.push(horizontal, vertical);
                if !directions.is_empty() {
                    self.wake_cursor();
                }
                for direction in directions {
                    let action = match direction {
                        WheelDirection::Up => "up",
                        WheelDirection::Down => "down",
                        WheelDirection::Left => "left",
                        WheelDirection::Right => "right",
                    };
                    self.send_mouse("wheel", action);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        for path in platform::take_open_files() {
            self.open_file(&path);
        }
        self.process_rpc_events(event_loop);
        if self.cursor_blink.advance(Instant::now())
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(8)));
    }
}

fn window_theme(theme: WindowTheme) -> Option<Theme> {
    match theme {
        WindowTheme::Auto => None,
        WindowTheme::Light => Some(Theme::Light),
        WindowTheme::Dark => Some(Theme::Dark),
    }
}

fn mado_window_icon() -> Option<Icon> {
    #[cfg(target_os = "windows")]
    {
        const ICON_RGBA: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/packaging/icons/mado-256.rgba"
        ));
        Icon::from_rgba(ICON_RGBA.to_vec(), 256, 256).ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn mode_allows_ime(mode: &str) -> bool {
    mode.starts_with("insert")
        || mode.starts_with("replace")
        || mode.starts_with("cmdline")
        || mode.starts_with("terminal")
}

fn close_command() -> String {
    "<Esc>:confirm qa<CR>".to_owned()
}

fn window_title(has_received_first_frame: bool) -> &'static str {
    if has_received_first_frame {
        "Mado"
    } else {
        "Mado — Starting…"
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorBlink, close_command, mode_allows_ime, window_title};
    use crate::nvim::redraw::CursorStyle;
    use std::time::{Duration, Instant};

    #[test]
    fn enables_ime_only_for_text_entry_modes() {
        for mode in [
            "insert",
            "insert_completion",
            "replace",
            "cmdline_normal",
            "cmdline_insert",
            "terminal",
        ] {
            assert!(mode_allows_ime(mode), "{mode}");
        }
        for mode in ["normal", "visual", "operator", "select"] {
            assert!(!mode_allows_ime(mode), "{mode}");
        }
    }

    #[test]
    fn follows_cursor_blink_timing() {
        let start = Instant::now();
        let mut blink = CursorBlink::default();
        blink.reset(
            &CursorStyle {
                blink_wait: 100,
                blink_on: 200,
                blink_off: 50,
                ..CursorStyle::default()
            },
            start,
        );
        assert!(blink.visible);
        assert!(!blink.advance(start + Duration::from_millis(99)));
        assert!(blink.advance(start + Duration::from_millis(100)));
        assert!(!blink.visible);
        assert!(blink.advance(start + Duration::from_millis(150)));
        assert!(blink.visible);
    }

    #[test]
    fn close_uses_neovim_confirmation_flow() {
        assert_eq!(close_command(), "<Esc>:confirm qa<CR>");
    }

    #[test]
    fn window_title_reflects_startup_state() {
        assert_eq!(window_title(false), "Mado — Starting…");
        assert_eq!(window_title(true), "Mado");
    }
}
