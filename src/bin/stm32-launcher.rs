// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::Instant;

use glium::Surface;
use imgui::{Condition, Context, FontSource, Ui};
use imgui_glium_renderer::Renderer;
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use stm32_emulator::launcher::process::{
    discover_emulator, validate_firmware, OutputStream, RunningEmulator, TemporaryConfig,
};
use stm32_emulator::launcher::registry::{all_variants, support_summary};
use stm32_emulator::launcher::ui_state::LauncherState;
use stm32_emulator::launcher::workspace::{
    SavedLauncherState, WindowPlacement, WorkspaceStore,
};
use stm32_emulator::launcher::{
    EmulationSupport, KnownVariant, LauncherCpuModel, ResolvedProfile,
};

const BG: [f32; 4] = [0.086, 0.106, 0.133, 1.0];
const PANEL: [f32; 4] = [0.133, 0.165, 0.208, 1.0];
const AMBER: [f32; 4] = [0.949, 0.722, 0.294, 1.0];
const CYAN: [f32; 4] = [0.314, 0.769, 0.827, 1.0];
const RED: [f32; 4] = [0.878, 0.424, 0.459, 1.0];

#[derive(Default)]
struct ManualForm {
    enabled: bool,
    cpu_model: LauncherCpuModel,
    svd: String,
    vector_table: String,
    flash_start: String,
    flash_size: String,
    ram_start: String,
    ram_size: String,
}

struct App {
    state: LauncherState,
    variants: Vec<KnownVariant>,
    filter: String,
    manual: ManualForm,
    temporary_config: Option<TemporaryConfig>,
    process: Option<RunningEmulator>,
}

impl App {
    fn new(saved: SavedLauncherState) -> Self {
        Self {
            state: LauncherState {
                firmware: saved.firmware,
                svd: saved.svd,
                emulator_executable: saved.emulator_executable,
                selected_variant: saved.selected_variant,
                ..Default::default()
            },
            variants: all_variants(),
            filter: saved.filter,
            manual: ManualForm {
                enabled: saved.manual_enabled,
                cpu_model: saved.manual_cpu_model,
                svd: saved.manual_svd,
                vector_table: if saved.manual_vector_table.is_empty() { "0x08000000".to_owned() } else { saved.manual_vector_table },
                flash_start: if saved.manual_flash_start.is_empty() { "0x08000000".to_owned() } else { saved.manual_flash_start },
                flash_size: if saved.manual_flash_size.is_empty() { "0x00100000".to_owned() } else { saved.manual_flash_size },
                ram_start: if saved.manual_ram_start.is_empty() { "0x20000000".to_owned() } else { saved.manual_ram_start },
                ram_size: if saved.manual_ram_size.is_empty() { "0x00020000".to_owned() } else { saved.manual_ram_size },
            },
            temporary_config: None,
            process: None,
        }
    }

    fn saved_state(&self) -> SavedLauncherState {
        SavedLauncherState {
            firmware: self.state.firmware.clone(),
            svd: self.state.svd.clone(),
            emulator_executable: self.state.emulator_executable.clone(),
            selected_variant: self.state.selected_variant.clone(),
            filter: self.filter.clone(),
            manual_enabled: self.manual.enabled,
            manual_cpu_model: self.manual.cpu_model,
            manual_svd: self.manual.svd.clone(),
            manual_vector_table: self.manual.vector_table.clone(),
            manual_flash_start: self.manual.flash_start.clone(),
            manual_flash_size: self.manual.flash_size.clone(),
            manual_ram_start: self.manual.ram_start.clone(),
            manual_ram_size: self.manual.ram_size.clone(),
        }
    }

    fn selected_variant(&self) -> Option<KnownVariant> {
        let id = self.state.selected_variant.as_deref()?;
        self.variants
            .iter()
            .copied()
            .find(|variant| variant.id == id)
    }

    fn select_variant(&mut self, variant: KnownVariant) {
        self.manual.enabled = false;
        self.state.selected_variant = Some(variant.id.to_owned());
        if variant.id == "proteus_f7" && self.state.svd.is_none() {
            self.state.svd = Some(
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("proteus_f7")
                    .join("STM32F767.svd"),
            );
        }
    }

    fn resolved_profile(&self) -> Result<ResolvedProfile, String> {
        let firmware = self
            .state
            .firmware
            .clone()
            .ok_or_else(|| "Choose a firmware .bin file.".to_owned())?;
        validate_firmware(&firmware).map_err(|error| error.to_string())?;

        if self.manual.enabled {
            let svd = PathBuf::from(&self.manual.svd);
            if !svd.is_file() {
                return Err("Manual profile requires an existing SVD file.".to_owned());
            }
            return Ok(ResolvedProfile::manual(
                self.manual.cpu_model,
                firmware,
                svd,
                parse_address(&self.manual.vector_table, "vector table")?,
                parse_address(&self.manual.flash_start, "flash start")?,
                parse_address(&self.manual.flash_size, "flash size")?,
                parse_address(&self.manual.ram_start, "RAM start")?,
                parse_address(&self.manual.ram_size, "RAM size")?,
            ));
        }

        let variant = self
            .selected_variant()
            .ok_or_else(|| "Choose a cataloged variant or enable Manual profile.".to_owned())?;
        if variant.support == EmulationSupport::Unsupported {
            return Err(support_summary(variant).to_owned());
        }
        let svd = self
            .state
            .svd
            .clone()
            .ok_or_else(|| "Choose the SVD asset required by this profile.".to_owned())?;
        if !svd.is_file() {
            return Err(format!("SVD file does not exist: {}", svd.display()));
        }
        ResolvedProfile::for_variant(variant, firmware, svd).map_err(|error| error.to_string())
    }

    fn can_run(&self) -> bool {
        self.state.can_run() && self.resolved_profile().is_ok()
    }

    fn start(&mut self) {
        let result = (|| {
            let profile = self.resolved_profile()?;
            let yaml = profile.to_yaml().map_err(|error| error.to_string())?;
            let temporary_config =
                TemporaryConfig::write(&yaml).map_err(|error| error.to_string())?;
            let executable =
                discover_emulator(self.state.emulator_executable.as_deref()).map_err(|error| {
                    format!("{error}. Use “Choose emulator” to select it explicitly.")
                })?;
            let process = RunningEmulator::spawn(&executable, temporary_config.path(), 1)
                .map_err(|error| error.to_string())?;
            self.temporary_config = Some(temporary_config);
            self.process = Some(process);
            self.state.running = true;
            Ok::<(), String>(())
        })();
        self.state.last_error = result.err();
    }

    fn stop(&mut self) {
        if let Some(process) = self.process.as_mut() {
            if let Err(error) = process.stop() {
                self.state.last_error = Some(error.to_string());
            }
        }
        self.process = None;
        self.temporary_config = None;
        self.state.running = false;
    }

    fn refresh_process(&mut self) {
        if let Some(process) = self.process.as_mut() {
            process.poll_output();
            match process.is_running() {
                Ok(true) => {}
                Ok(false) => self.state.running = false,
                Err(error) => {
                    self.state.last_error = Some(error.to_string());
                    self.state.running = false;
                }
            }
        }
    }
}

fn parse_address(input: &str, label: &str) -> Result<u32, String> {
    let value = input.trim();
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|hex| u32::from_str_radix(hex, 16))
        .unwrap_or_else(|| value.parse());
    parsed.map_err(|_| format!("Manual {label} must be decimal or hexadecimal (0x...)."))
}

fn window_attributes(
    placement: Option<WindowPlacement>,
) -> glium::winit::window::WindowAttributes {
    let placement = placement.filter(|value| value.width > 0 && value.height > 0);
    let mut attributes = glium::winit::window::Window::default_attributes()
        .with_title("STM32 Emulator")
        .with_inner_size(glium::winit::dpi::PhysicalSize::new(
            placement.as_ref().map_or(1440, |value| value.width),
            placement.as_ref().map_or(900, |value| value.height),
        ));
    if let Some(placement) = placement {
        attributes = attributes.with_position(glium::winit::dpi::PhysicalPosition::new(
            placement.x,
            placement.y,
        ));
    }
    attributes
}

fn main() {
    let store = WorkspaceStore::for_current_user().expect("launcher workspace directory");
    let mut workspace = store.load().unwrap_or_default();
    let event_loop = glium::winit::event_loop::EventLoop::builder()
        .build()
        .expect("event loop building");
    let (window, display) = glium::backend::glutin::SimpleWindowBuilder::new()
        .with_title("STM32 Emulator — Firmware Launcher")
        .set_window_builder(window_attributes(workspace.window.clone()))
        .build(&event_loop);
    let (mut platform, mut imgui) = imgui_init(&window, store.imgui_ini_path());
    let mut renderer = Renderer::new(&mut imgui, &display).expect("ImGui renderer initialization");
    let mut app = App::new(workspace.state.clone());
    let mut last_frame = Instant::now();

    #[allow(deprecated)]
    event_loop
        .run(move |event, window_target| match event {
            glium::winit::event::Event::NewEvents(_) => {
                let now = Instant::now();
                imgui.io_mut().update_delta_time(now - last_frame);
                last_frame = now;
            }
            glium::winit::event::Event::AboutToWait => {
                platform
                    .prepare_frame(imgui.io_mut(), &window)
                    .expect("preparing ImGui frame");
                window.request_redraw();
            }
            glium::winit::event::Event::WindowEvent {
                event: glium::winit::event::WindowEvent::RedrawRequested,
                ..
            } => {
                app.refresh_process();
                let ui = imgui.frame();
                ui.dockspace_over_main_viewport();
                draw_signal_chain(ui, &app);
                draw_firmware_panel(ui, &mut app);
                draw_configuration_panel(ui, &mut app);
                draw_notes_panel(ui, &app);
                draw_output_panel(ui, &mut app);
                workspace.state = app.saved_state();

                let mut target = display.draw();
                target.clear_color_srgb(BG[0], BG[1], BG[2], BG[3]);
                platform.prepare_render(ui, &window);
                let draw_data = imgui.render();
                renderer
                    .render(&mut target, draw_data)
                    .expect("rendering ImGui frame");
                target.finish().expect("swapping launcher frame");
            }
            glium::winit::event::Event::WindowEvent {
                event: glium::winit::event::WindowEvent::CloseRequested,
                ..
            } => {
                workspace.state = app.saved_state();
                let size = window.inner_size();
                let position = window.outer_position().unwrap_or_default();
                workspace.window = Some(WindowPlacement {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                });
                let _ = store.save(&workspace);
                window_target.exit()
            },
            glium::winit::event::Event::WindowEvent {
                event: glium::winit::event::WindowEvent::Resized(size),
                ..
            } => {
                if size.width > 0 && size.height > 0 {
                    display.resize(size.into());
                }
                platform.handle_event(imgui.io_mut(), &window, &event);
            }
            event => platform.handle_event(imgui.io_mut(), &window, &event),
        })
        .expect("event loop error");
}

fn imgui_init(
    window: &glium::winit::window::Window,
    ini_path: PathBuf,
) -> (WinitPlatform, Context) {
    let mut imgui = Context::create();
    imgui.set_ini_filename(Some(ini_path));
    imgui
        .io_mut()
        .config_flags
        .insert(imgui::ConfigFlags::DOCKING_ENABLE);
    imgui.style_mut().window_rounding = 3.0;
    imgui.style_mut().colors[imgui::StyleColor::WindowBg as usize] = BG;
    imgui.style_mut().colors[imgui::StyleColor::ChildBg as usize] = PANEL;
    imgui
        .fonts()
        .add_font(&[FontSource::DefaultFontData { config: None }]);

    let mut platform = WinitPlatform::new(&mut imgui);
    platform.attach_window(imgui.io_mut(), window, HiDpiMode::Default);
    (platform, imgui)
}

fn draw_signal_chain(ui: &Ui, app: &App) {
    ui.window("Signal Chain")
        .position([12.0, 12.0], Condition::FirstUseEver)
        .size([640.0, 66.0], Condition::FirstUseEver)
        .build(|| {
            let firmware = app.state.firmware.is_some();
            let variant = app.manual.enabled || app.selected_variant().is_some();
            let profile = app.resolved_profile().is_ok();
            indicator(ui, "1  Firmware", firmware);
            ui.same_line();
            ui.text("→");
            ui.same_line();
            indicator(ui, "2  Variant", variant);
            ui.same_line();
            ui.text("→");
            ui.same_line();
            indicator(ui, "3  Profile", profile);
            ui.same_line();
            ui.text("→");
            ui.same_line();
            indicator(ui, "4  Emulator", app.state.running);
        });
}

fn indicator(ui: &Ui, label: &str, active: bool) {
    ui.text_colored(if active { CYAN } else { AMBER }, label);
}

fn draw_firmware_panel(ui: &Ui, app: &mut App) {
    ui.window("Firmware & Variant")
        .size([430.0, 560.0], Condition::FirstUseEver)
        .build(|| {
            if ui.button("Choose firmware .bin") {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Firmware", &["bin"])
                    .pick_file()
                {
                    app.state.firmware = Some(path);
                }
            }
            ui.same_line();
            ui.text_disabled(display_path(app.state.firmware.as_deref()));

            if ui.button("Choose emulator") {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    app.state.emulator_executable = Some(path);
                }
            }
            ui.same_line();
            ui.text_disabled(display_path(app.state.emulator_executable.as_deref()));

            ui.separator();
            ui.checkbox("Manual profile", &mut app.manual.enabled);
            if app.manual.enabled {
                app.state.selected_variant = None;
                ui.text_colored(AMBER, "Manual mode: every address below must be explicit.");
                return;
            }

            ui.input_text("Filter", &mut app.filter).build();
            let filter = app.filter.to_lowercase();
            let selected = app.state.selected_variant.clone();
            let visible_variants: Vec<_> = app
                .variants
                .iter()
                .copied()
                .filter(|variant| {
                    filter.is_empty()
                        || variant.id.to_lowercase().contains(&filter)
                        || variant.display_name.to_lowercase().contains(&filter)
                })
                .collect();
            if let Some(_list) = ui
                .child_window("Variant catalog")
                .size([0.0, 330.0])
                .begin()
            {
                for variant in visible_variants {
                    let label = format!("{}  [{}]", variant.id, support_label(variant.support));
                    if ui
                        .selectable_config(&label)
                        .selected(selected.as_deref() == Some(variant.id))
                        .build()
                    {
                        app.select_variant(variant);
                    }
                    ui.same_line();
                    ui.text_disabled(variant.mcu.unwrap_or("MCU unknown"));
                }
            }
        });
}

fn draw_configuration_panel(ui: &Ui, app: &mut App) {
    ui.window("Resolved Configuration")
        .size([520.0, 560.0], Condition::FirstUseEver)
        .build(|| {
            if app.manual.enabled {
                ui.text("CPU model");
                if ui.radio_button_bool(
                    "Cortex-M4",
                    app.manual.cpu_model == LauncherCpuModel::CortexM4,
                ) {
                    app.manual.cpu_model = LauncherCpuModel::CortexM4;
                }
                ui.same_line();
                if ui.radio_button_bool(
                    "Cortex-M7",
                    app.manual.cpu_model == LauncherCpuModel::CortexM7,
                ) {
                    app.manual.cpu_model = LauncherCpuModel::CortexM7;
                }
                ui.input_text("SVD path", &mut app.manual.svd).build();
                ui.input_text("Vector table", &mut app.manual.vector_table)
                    .build();
                ui.input_text("Flash start", &mut app.manual.flash_start)
                    .build();
                ui.input_text("Flash size", &mut app.manual.flash_size)
                    .build();
                ui.input_text("RAM start", &mut app.manual.ram_start)
                    .build();
                ui.input_text("RAM size", &mut app.manual.ram_size).build();
            } else {
                if ui.button("Choose SVD") {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("SVD", &["svd"])
                        .pick_file()
                    {
                        app.state.svd = Some(path);
                    }
                }
                ui.same_line();
                ui.text_disabled(display_path(app.state.svd.as_deref()));
            }

            ui.separator();
            match app.resolved_profile() {
                Ok(profile) => {
                    ui.text_colored(CYAN, "Profile is evidence-backed and ready to launch.");
                    match profile.to_yaml() {
                        Ok(yaml) => {
                            ui.child_window("Generated YAML")
                                .size([0.0, 300.0])
                                .build(|| ui.text_wrapped(yaml));
                        }
                        Err(error) => ui.text_colored(RED, error.to_string()),
                    }
                }
                Err(error) => {
                    ui.text_colored(RED, "Run blocked");
                    ui.text_wrapped(error);
                }
            }
        });
}

fn draw_notes_panel(ui: &Ui, app: &App) {
    ui.window("Hardware Notes")
        .size([420.0, 300.0], Condition::FirstUseEver)
        .build(|| {
            if app.manual.enabled {
                ui.text_colored(AMBER, "Manual profile");
                ui.text_wrapped(
                    "Only the entered flash and RAM regions will be mapped. Peripheral and device behavior is not inferred.",
                );
                return;
            }
            match app.selected_variant() {
                Some(variant) => {
                    ui.text_colored(
                        if variant.support == EmulationSupport::Unsupported {
                            AMBER
                        } else {
                            CYAN
                        },
                        format!("{} — {}", variant.display_name, support_label(variant.support)),
                    );
                    ui.separator();
                    ui.text_wrapped(support_summary(variant));
                    if variant.id == "proteus_f7" {
                        ui.text_wrapped(
                            "Verified: STM32F767 code and AXI flash aliases, ITCM/DTCM/SRAM map, and F767 SVD. Current firmware trace still reaches an unmodeled FLASH ACR latency startup boundary.",
                        );
                    } else {
                        ui.text_wrapped(
                            "This entry is selectable because it is known source data. It remains blocked until its MCU, SVD, memory map, and required devices are independently verified.",
                        );
                    }
                }
                None => ui.text_wrapped("Select a cataloged variant to inspect its emulator evidence."),
            }
        });
}

fn draw_output_panel(ui: &Ui, app: &mut App) {
    ui.window("Emulator Output")
        .size([760.0, 360.0], Condition::FirstUseEver)
        .build(|| {
            let run_enabled = app.can_run();
            let mut run_clicked = false;
            ui.disabled(!run_enabled, || {
                run_clicked = ui.button("Run emulator");
            });
            ui.same_line();
            if ui.button("Stop") {
                app.stop();
            }
            ui.same_line();
            ui.text_colored(
                if app.state.running { CYAN } else { AMBER },
                if app.state.running { "Running" } else { "Idle" },
            );
            if run_clicked {
                app.start();
            }
            if let Some(error) = &app.state.last_error {
                ui.separator();
                ui.text_colored(RED, error);
            }

            let output = app
                .process
                .as_ref()
                .map(|process| process.output().iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            ui.child_window("Trace").size([0.0, 250.0]).build(|| {
                if output.is_empty() {
                    ui.text_disabled("No emulator output yet.");
                }
                for line in output {
                    ui.text_colored(
                        if line.stream == OutputStream::Stderr {
                            RED
                        } else {
                            CYAN
                        },
                        line.text,
                    );
                }
            });
        });
}

fn support_label(support: EmulationSupport) -> &'static str {
    match support {
        EmulationSupport::Runnable => "runnable",
        EmulationSupport::Partial => "partial",
        EmulationSupport::Unsupported => "cataloged",
    }
}

fn display_path(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "not selected".to_owned())
}
