use crate::{
    android::backend::wayland::{
        compositor::{send_frames_surface_tree, ClientState, State},
        element::{WindowElement, WindowRenderElement},
        CentralizedEvent, WaylandBackend,
    },
    android::proot::process::ArchProcess,
    core::logging::PolarBearExpectation,
};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, Event, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, TouchEvent,
};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement, WaylandSurfaceTexture,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Color32F, Frame, Renderer};
use smithay::desktop::{Space, Window};
use smithay::input::keyboard::FilterResult;
use smithay::input::{pointer, touch};
use smithay::reexports::wayland_server::protocol::wl_pointer::ButtonState;
use smithay::utils::{Logical, Point, Rectangle, Transform, SERIAL_COUNTER};
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};
use smithay::wayland::compositor;
use smithay::backend::renderer::element::{
    texture::TextureRenderElement,
    Id,
};
use smithay::backend::renderer::gles::GlesTexture;
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;

fn calculate_dynamic_scale_factor(screen_size: smithay::utils::Size<i32, smithay::utils::Physical>, device_scale_factor: f64) -> f64 {
    let screen_width = screen_size.w as f64;
    let screen_height = screen_size.h as f64;
    
    let control_panel_height = 120.0 * device_scale_factor;
    let available_height = screen_height - control_panel_height;
    
    let calc_native_width = 600.0;
    let calc_native_height = 800.0;
    
    let width_scale = (screen_width * 0.60) / calc_native_width;
    let height_scale = available_height / calc_native_height;
    
    let calculated_scale = width_scale.min(height_scale);
    
    (calculated_scale * device_scale_factor * 0.4).clamp(0.1, 0.6)
}

fn get_surface_app_id(surface: &ToplevelSurface) -> Option<String> {
    compositor::with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok())
            .and_then(|attributes| attributes.app_id.clone())
    })
}

fn get_surface_title(surface: &ToplevelSurface) -> Option<String> {
    compositor::with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok())
            .and_then(|attributes| attributes.title.clone())
    })
}

fn spawn_application(command: &str) {
    let cmd = command.to_string();
    std::thread::spawn(move || {
        let process = ArchProcess::exec(&cmd);
        if let Some(child) = process.process {
            log::info!("Started application: {}", cmd);
            std::mem::forget(child);
        } else {
            log::error!("Failed to spawn application: {}", cmd);
        }
    });
}


/**
 * As we currently use Xwayland, there is only 1 surface
 */
fn get_surface(state: &State) -> Option<ToplevelSurface> {
    if state.show_terminal {
        for surface in state.xdg_shell_state.toplevel_surfaces() {
            let app_id = get_surface_app_id(surface);
            let title = get_surface_title(surface);
            
            let is_terminal = app_id.as_ref().map_or(false, |id| id.contains("weston-terminal") || id.contains("terminal")) ||
                             title.as_ref().map_or(false, |t| t.contains("Terminal"));
            
            if is_terminal {
                return Some(surface.clone());
            }
        }
    } else if state.show_calculator {
        for surface in state.xdg_shell_state.toplevel_surfaces() {
            let app_id = get_surface_app_id(surface);
            let title = get_surface_title(surface);
            
            let is_calculator = app_id.as_ref().map_or(false, |id| id.contains("kcalc") || id.contains("calculator")) ||
                               title.as_ref().map_or(false, |t| t.contains("Calculator") || t.contains("KCalc"));
            
            if is_calculator {
                return Some(surface.clone());
            }
        }
    }
    
    state
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .next()
        .cloned()
}

fn clamp_coords(space: &Space<WindowElement>, pos: Point<f64, Logical>) -> Point<f64, Logical> {
    if space.outputs().next().is_none() {
        return pos;
    }

    let (pos_x, pos_y) = pos.into();
    let max_x = space
        .outputs()
        .fold(0, |acc, o| acc + space.output_geometry(o).unwrap().size.w);
    let clamped_x = pos_x.clamp(0.0, max_x as f64);
    let max_y = space
        .outputs()
        .find(|o| {
            let geo = space.output_geometry(o).unwrap();
            geo.contains((clamped_x as i32, 0))
        })
        .map(|o| space.output_geometry(o).unwrap().size.h);

    if let Some(max_y) = max_y {
        let clamped_y = pos_y.clamp(0.0, max_y as f64);
        (clamped_x, clamped_y).into()
    } else {
        (clamped_x, pos_y).into()
    }
}

pub fn handle(event: CentralizedEvent, backend: &mut WaylandBackend, event_loop: &ActiveEventLoop) {
    match event {
        CentralizedEvent::CloseRequested => {
            log::info!("The close button was pressed; stopping");
            event_loop.exit();
        }
        CentralizedEvent::Redraw => {
            if let Some(winit) = backend.graphic_renderer.as_mut() {
                let size = winit.window_size();
                log::info!("Screen size: {}x{}, device scale: {:.2}", size.w, size.h, backend.scale_factor);
                let damage = Rectangle::from_size(size);
                {
                    let (renderer, mut framebuffer) = winit.bind().unwrap();

                    let compositor = &mut backend.compositor;

                    let mut elements: Vec<WindowRenderElement<GlowRenderer>> = Vec::new();

                    compositor.state.terminal_surface_elements.clear();
                    compositor.state.calculator_surface_elements.clear();
                    
                    if compositor.state.show_terminal {
                        log::info!("Looking for terminal surfaces...");
                        for surface in compositor.state.xdg_shell_state.toplevel_surfaces() {
                            let app_id = get_surface_app_id(surface);
                            let title = get_surface_title(surface);
                            log::info!("Surface found - app_id: {:?}, title: {:?}", app_id, title);
                            
                            let is_terminal = app_id.as_ref().map_or(false, |id| id.contains("weston-terminal") || id.contains("terminal")) ||
                                             title.as_ref().map_or(false, |t| t.contains("Terminal"));
                            
                            if !is_terminal {
                                continue;
                            }
                            
                            log::info!("Processing terminal surface with app_id: {:?}, title: {:?}", app_id, title);
                            
                            let window = Window::new_wayland_window(surface.clone());
                            window.override_z_index(60);
                            
                            let panel_height = (50.0 * backend.scale_factor.max(1.0)) as i32;
                            let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = 
                                render_elements_from_surface_tree(
                                    renderer,
                                    surface.wl_surface(),
                                    (0, panel_height),
                                    1.0,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            
                            let surface_elements_copy: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = 
                                render_elements_from_surface_tree(
                                    renderer,
                                    surface.wl_surface(),
                                    (0, panel_height),
                                    1.0,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            
                            for element in surface_elements {
                                elements.push(WindowRenderElement::Window(element));
                            }
                            
                            compositor.state.terminal_surface_elements.extend(surface_elements_copy);
                        }
                    }
                    
                    if compositor.state.show_calculator {
                        log::info!("Looking for calculator surfaces...");
                        let dynamic_scale = calculate_dynamic_scale_factor(size, backend.scale_factor);
                        for surface in compositor.state.xdg_shell_state.toplevel_surfaces() {
                            let app_id = get_surface_app_id(surface);
                            let title = get_surface_title(surface);
                            log::info!("Surface found - app_id: {:?}, title: {:?}", app_id, title);
                            
                            let is_calculator = app_id.as_ref().map_or(false, |id| id.contains("kcalc") || id.contains("calculator")) ||
                                               title.as_ref().map_or(false, |t| t.contains("Calculator") || t.contains("KCalc"));
                            
                            if !is_calculator {
                                continue;
                            }
                            
                            log::info!("Processing calculator surface with app_id: {:?}, title: {:?}", app_id, title);
                            
                            let window = Window::new_wayland_window(surface.clone());
                            window.override_z_index(60);
                            
                            let panel_height = (50.0 * backend.scale_factor.max(1.0)) as i32;
                            let surface_elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = 
                                render_elements_from_surface_tree(
                                    renderer,
                                    surface.wl_surface(),
                                    (0, panel_height),
                                    dynamic_scale,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            
                            let surface_elements_copy: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = 
                                render_elements_from_surface_tree(
                                    renderer,
                                    surface.wl_surface(),
                                    (0, panel_height),
                                    dynamic_scale,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            
                            for element in surface_elements {
                                elements.push(WindowRenderElement::Window(element));
                            }
                            
                            compositor.state.calculator_surface_elements.extend(surface_elements_copy);
                        }
                    }

                    let scale_factor = backend.scale_factor.max(1.0);


                    if let Ok(Some(egui_element)) = compositor.state.egui_state.render(
                        renderer,
                        |ctx| {
                            let mut style = (*ctx.style()).clone();
                            style.text_styles.insert(
                                egui::TextStyle::Body,
                                egui::FontId::new(48.0 * scale_factor as f32, egui::FontFamily::Proportional),
                            );
                            style.text_styles.insert(
                                egui::TextStyle::Button,
                                egui::FontId::new(18.0 * scale_factor as f32, egui::FontFamily::Proportional),
                            );
                            style.text_styles.insert(
                                egui::TextStyle::Heading,
                                egui::FontId::new(48.0 * scale_factor as f32, egui::FontFamily::Proportional),
                            );
                            ctx.set_style(style);
                            
                            egui::TopBottomPanel::top("control_panel")
                                .exact_height(50.0 * scale_factor as f32)
                                .show(ctx, |ui| {
                                    ui.horizontal_centered(|ui| {
                                        ui.style_mut().text_styles.insert(
                                            egui::TextStyle::Button,
                                            egui::FontId::new(14.0 * scale_factor as f32, egui::FontFamily::Proportional),
                                        );
                                        
                                        if ui.add_sized([80.0 * scale_factor as f32, 60.0 * scale_factor as f32], egui::Button::new("term")).clicked() {
                                            log::info!("Terminal button clicked!");
                                            compositor.state.show_terminal = !compositor.state.show_terminal;
                                            
                                            if compositor.state.show_terminal {
                                                compositor.state.show_calculator = false;
                                                if !compositor.state.terminal_spawned {
                                                    spawn_application("WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/tmp GDK_SCALE=1.5 GDK_DPI_SCALE=1.5 FONTCONFIG_PATH=/tmp/fontconfig weston-terminal");
                                                    compositor.state.terminal_spawned = true;
                                                }
                                            } else if !compositor.state.show_terminal && compositor.state.terminal_spawned {
                                                spawn_application("pkill weston-terminal");
                                                compositor.state.terminal_spawned = false;
                                            }
                                        }
                                        
                                        ui.separator();
                                        
                                        if ui.add_sized([80.0 * scale_factor as f32, 60.0 * scale_factor as f32], egui::Button::new("calc")).clicked() {
                                            log::info!("Calculator button clicked!");
                                            compositor.state.show_calculator = !compositor.state.show_calculator;
                                            
                                            if compositor.state.show_calculator {
                                                compositor.state.show_terminal = false;
                                                if !compositor.state.calculator_spawned {
                                                    let dynamic_scale = calculate_dynamic_scale_factor(size, backend.scale_factor);
                                                    let qt_scale = (dynamic_scale * 1.2).clamp(0.6, 1.5);
                                                    let font_dpi = (96.0 * qt_scale) as i32;
                                                    
                                                    let spawn_command = format!(
                                                        "WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/tmp QT_SCALE_FACTOR={:.2} QT_AUTO_SCREEN_SCALE_FACTOR=0 QT_FONT_DPI={} QT_WAYLAND_FORCE_DPI={} QT_ENABLE_HIGHDPI_SCALING=0 QT_SCREEN_SCALE_FACTORS={:.2} GDK_SCALE={:.2} GDK_DPI_SCALE={:.2} FONTCONFIG_PATH=/tmp/fontconfig FREETYPE_PROPERTIES=truetype:interpreter-version=40 kcalc",
                                                        qt_scale, font_dpi, font_dpi, qt_scale, qt_scale, qt_scale
                                                    );
                                                    spawn_application(&spawn_command);
                                                    compositor.state.calculator_spawned = true;
                                                }
                                            } else if !compositor.state.show_calculator && compositor.state.calculator_spawned {
                                                spawn_application("pkill kcalc");
                                                compositor.state.calculator_spawned = false;
                                            }
                                        }
                                    });
                                });
                        },
                        Rectangle::from_size((size.w, size.h).into()),
                        scale_factor,
                        0.9,
                    ) {
                        elements.push(WindowRenderElement::Egui(egui_element));
                    }

                    let frame_result = renderer.render(&mut framebuffer, size, Transform::Flipped180);
                    let mut frame = match frame_result {
                        Ok(frame) => frame,
                        Err(err) => {
                            log::error!("Failed to create render frame: {:?}", err);
                            return;
                        }
                    };
                    frame
                        .clear(Color32F::new(0.1, 0.0, 0.0, 1.0), &[damage])
                        .unwrap();
                    
                    draw_render_elements(&mut frame, 1.0, &elements, &[damage]).unwrap();

                    let _ = frame.finish().unwrap();

                    for surface in compositor.state.xdg_shell_state.toplevel_surfaces() {
                        send_frames_surface_tree(
                            surface.wl_surface(),
                            compositor.start_time.elapsed().as_millis() as u32,
                        );
                    }

                    let all_surfaces = compositor.state.xdg_shell_state.toplevel_surfaces();
                    
                    if compositor.state.show_terminal && !compositor.state.terminal_spawned {
                        let has_terminal_surface = all_surfaces.iter().any(|surface| {
                            let app_id = get_surface_app_id(surface);
                            let title = get_surface_title(surface);
                            app_id.as_ref().map_or(false, |id| id.contains("weston-terminal") || id.contains("terminal")) ||
                            title.as_ref().map_or(false, |t| t.contains("Terminal"))
                        });
                        if has_terminal_surface {
                            compositor.state.terminal_spawned = true;
                        }
                    }
                    
                    if compositor.state.show_calculator && !compositor.state.calculator_spawned {
                        let has_calculator_surface = all_surfaces.iter().any(|surface| {
                            let app_id = get_surface_app_id(surface);
                            let title = get_surface_title(surface);
                            app_id.as_ref().map_or(false, |id| id.contains("kcalc") || id.contains("calculator")) ||
                            title.as_ref().map_or(false, |t| t.contains("Calculator") || t.contains("KCalc"))
                        });
                        if has_calculator_surface {
                            compositor.state.calculator_spawned = true;
                        }
                    }

                    if let Some(stream) = compositor
                        .listener
                        .accept()
                        .pb_expect("Failed to accept listener")
                    {
                        log::info!("Got a client: {:?}", stream);

                        let client = compositor
                            .display
                            .handle()
                            .insert_client(stream, Arc::new(ClientState::default()))
                            .unwrap();
                        compositor.clients.push(client);
                    }

                    compositor
                        .display
                        .dispatch_clients(&mut compositor.state)
                        .pb_expect("Failed to dispatch clients");
                    compositor
                        .display
                        .flush_clients()
                        .pb_expect("Failed to flush clients");
                }

                // It is important that all events on the display have been dispatched and flushed to clients before
                // swapping buffers because this operation may block.
                winit.submit(Some(&[damage])).unwrap();
            }

            // Redraw the application.
            //
            // It's preferable for applications that do not render continuously to render in
            // this event rather than in AboutToWait, since rendering in here allows
            // the program to gracefully handle redraws requested by the OS.

            // Draw.

            // Queue a RedrawRequested event.
            //
            // You only need to call this if you've determined that you need to redraw in
            // applications which do not always need to. Applications that redraw continuously
            // can render here instead.
            backend
                .graphic_renderer
                .as_ref()
                .unwrap()
                .window()
                .request_redraw();
        }
        CentralizedEvent::Input(event) => match event {
            InputEvent::Keyboard { event } => {
                let compositor = &mut backend.compositor;
                let state = &mut compositor.state;
                
                let all_surfaces = state.xdg_shell_state.toplevel_surfaces();
                let mut target_surface = None;
                
                if state.show_terminal {
                    target_surface = all_surfaces.iter().find(|surface| {
                        let app_id = get_surface_app_id(surface);
                        let title = get_surface_title(surface);
                        app_id.as_ref().map_or(false, |id| id.contains("weston-terminal") || id.contains("terminal")) ||
                        title.as_ref().map_or(false, |t| t.contains("Terminal"))
                    });
                } else if state.show_calculator {
                    target_surface = all_surfaces.iter().find(|surface| {
                        let app_id = get_surface_app_id(surface);
                        let title = get_surface_title(surface);
                        app_id.as_ref().map_or(false, |id| id.contains("kcalc") || id.contains("calculator")) ||
                        title.as_ref().map_or(false, |t| t.contains("Calculator") || t.contains("KCalc"))
                    });
                }
                
                if let Some(surface) = target_surface {
                    let surface_clone = surface.wl_surface().clone();
                    compositor.keyboard.set_focus(state, Some(surface_clone), SERIAL_COUNTER.next_serial());
                } else {
                    compositor.keyboard.set_focus(state, None, SERIAL_COUNTER.next_serial());
                }
                
                let wants_keyboard = state.egui_state.wants_keyboard();
                let should_handle_egui = wants_keyboard || 
                                       (!state.show_terminal && !state.show_calculator);
                let egui_state = state.egui_state.clone();
                let key_pressed = event.state() == smithay::backend::input::KeyState::Pressed;
                
                compositor.keyboard.input::<(), _>(
                    state,
                    event.key_code(),
                    event.state(),
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                    move |_data, modifiers, handle| {
                        if should_handle_egui {
                            egui_state.handle_keyboard(&handle, key_pressed, *modifiers);
                        }
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::TouchDown { event } => {
                let compositor = &mut backend.compositor;
                
                let max_x = compositor.state.space
                    .outputs()
                    .fold(0, |acc, o| acc + compositor.state.space.output_geometry(o).unwrap().size.w);

                let max_h_output = compositor.state.space
                    .outputs()
                    .max_by_key(|o| compositor.state.space.output_geometry(o).unwrap().size.h)
                    .unwrap();

                let max_y = compositor.state.space.output_geometry(max_h_output).unwrap().size.h;
                
                let touch_location = Point::from((event.x_transformed(max_x), event.y_transformed(max_y)));
                
                let state = &mut compositor.state;
                let wants_pointer = state.egui_state.wants_pointer();
                let should_handle_egui = wants_pointer || (!state.show_terminal && !state.show_calculator);
                
                if should_handle_egui {
                    state.egui_state.handle_pointer_motion(touch_location);
                    state.egui_state.handle_pointer_button(
                        smithay::backend::input::MouseButton::Left, 
                        true
                    );
                } else {
                    if let Some(surface) = get_surface(state) {
                        compositor.keyboard.set_focus(
                            state,
                            Some(surface.wl_surface().clone()),
                            SERIAL_COUNTER.next_serial(),
                        );
                        let serial = SERIAL_COUNTER.next_serial();
                        let time = event.time_msec();
                        compositor.touch.down(
                            state,
                            Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                            &touch::DownEvent {
                                slot: event.slot(),
                                location: (event.x_transformed(max_x), event.y_transformed(max_y)).into(),
                                serial,
                                time,
                            },
                        );
                        compositor.touch.frame(state);
                    }
                }
            }
            InputEvent::TouchUp { event } => {
                let compositor = &mut backend.compositor;
                let state = &mut compositor.state;
                
                let wants_pointer = state.egui_state.wants_pointer();
                let should_handle_egui = wants_pointer || (!state.show_terminal && !state.show_calculator);
                
                if should_handle_egui {
                    state.egui_state.handle_pointer_button(
                        smithay::backend::input::MouseButton::Left, 
                        false
                    );
                } else {
                    if let Some(_surface) = get_surface(state) {
                        let serial = SERIAL_COUNTER.next_serial();
                        let time = event.time_msec();
                        compositor.touch.up(
                            state,
                            &touch::UpEvent {
                                slot: event.slot(),
                                serial,
                                time,
                            },
                        );
                        compositor.touch.frame(state);
                    }
                }
            }
            InputEvent::TouchMotion { event } => {
                let compositor = &mut backend.compositor;
                
                let max_x = compositor.state.space
                    .outputs()
                    .fold(0, |acc, o| acc + compositor.state.space.output_geometry(o).unwrap().size.w);

                let max_h_output = compositor.state.space
                    .outputs()
                    .max_by_key(|o| compositor.state.space.output_geometry(o).unwrap().size.h)
                    .unwrap();

                let max_y = compositor.state.space.output_geometry(max_h_output).unwrap().size.h;
                
                let touch_location = Point::from((event.x_transformed(max_x), event.y_transformed(max_y)));
                
                let state = &mut compositor.state;
                let wants_pointer = state.egui_state.wants_pointer();
                let should_handle_egui = wants_pointer || (!state.show_terminal && !state.show_calculator);
                
                if should_handle_egui {
                    state.egui_state.handle_pointer_motion(touch_location);
                } else {
                    if let Some(surface) = get_surface(state) {
                        let time = event.time_msec();
                        compositor.touch.motion(
                            state,
                            Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                            &touch::MotionEvent {
                                slot: event.slot(),
                                location: (event.x_transformed(max_x), event.y_transformed(max_y)).into(),
                                time,
                            },
                        );
                        compositor.touch.frame(state);
                    }
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let compositor = &mut backend.compositor;
                let space = &compositor.state.space;
                let serial = SERIAL_COUNTER.next_serial();

                let max_x = space
                    .outputs()
                    .fold(0, |acc, o| acc + space.output_geometry(o).unwrap().size.w);

                let max_h_output = space
                    .outputs()
                    .max_by_key(|o| space.output_geometry(o).unwrap().size.h)
                    .unwrap();

                let max_y = space.output_geometry(max_h_output).unwrap().size.h;

                let mut pointer_location =
                    (event.x_transformed(max_x), event.y_transformed(max_y)).into();

                pointer_location = clamp_coords(space, pointer_location);
                
                compositor.state.egui_state.handle_pointer_motion(pointer_location);
                
                let wants_pointer = compositor.state.egui_state.wants_pointer();
                if !wants_pointer {
                    let pointer = compositor.pointer.clone();
                    if let Some(surface) = get_surface(&compositor.state) {
                        pointer.motion(
                            &mut compositor.state,
                            Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                            &pointer::MotionEvent {
                                location: pointer_location,
                                serial,
                                time: event.time_msec(),
                            },
                        );
                    }
                    pointer.frame(&mut compositor.state);
                }
            }
            InputEvent::PointerButton { event, .. } => {
                let compositor = &mut backend.compositor;
                let button = event.button_code();
                let button_state = ButtonState::from(event.state());
                
                let mouse_button = match button {
                    0x110 => smithay::backend::input::MouseButton::Left,
                    0x111 => smithay::backend::input::MouseButton::Right,
                    0x112 => smithay::backend::input::MouseButton::Middle,
                    0x115 => smithay::backend::input::MouseButton::Forward,
                    0x116 => smithay::backend::input::MouseButton::Back,
                    _ => smithay::backend::input::MouseButton::Left,
                };
                
                let wants_pointer = compositor.state.egui_state.wants_pointer();
                
                if wants_pointer {
                    compositor.state.egui_state.handle_pointer_button(
                        mouse_button, 
                        event.state() == smithay::backend::input::ButtonState::Pressed
                    );
                } else {
                    let serial = SERIAL_COUNTER.next_serial();
                    let pointer = compositor.pointer.clone();
                    
                    if let Some(surface) = get_surface(&compositor.state) {
                        compositor.keyboard.set_focus(
                            &mut compositor.state,
                            Some(surface.wl_surface().clone()),
                            SERIAL_COUNTER.next_serial(),
                        );
                    }
                    pointer.button(
                        &mut compositor.state,
                        &pointer::ButtonEvent {
                            button,
                            state: button_state.try_into().unwrap(),
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(&mut compositor.state);
                }
            }
            InputEvent::PointerAxis { event } => {
                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) / 120.);
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) / 120.);
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                {
                    let mut frame =
                        pointer::AxisFrame::new(event.time_msec()).source(event.source());
                    if horizontal_amount != 0.0 {
                        frame = frame.relative_direction(
                            Axis::Horizontal,
                            event.relative_direction(Axis::Horizontal),
                        );
                        frame = frame.value(Axis::Horizontal, horizontal_amount);
                        if let Some(discrete) = horizontal_amount_discrete {
                            frame = frame.v120(Axis::Horizontal, discrete as i32);
                        }
                    }
                    if vertical_amount != 0.0 {
                        frame = frame.relative_direction(
                            Axis::Vertical,
                            event.relative_direction(Axis::Vertical),
                        );
                        frame = frame.value(Axis::Vertical, vertical_amount);
                        if let Some(discrete) = vertical_amount_discrete {
                            frame = frame.v120(Axis::Vertical, discrete as i32);
                        }
                    }
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                    let compositor = &mut backend.compositor;
                    let pointer = compositor.pointer.clone();
                    pointer.axis(&mut compositor.state, frame);
                    pointer.frame(&mut compositor.state);
                }
            }
            _ => {}
        },
        _ => (),
    }
}
