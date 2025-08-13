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
use smithay::backend::renderer::{Color32F, Frame, Renderer, ExportMem};
use smithay::desktop::Space;
use smithay::input::keyboard::FilterResult;
use smithay::input::{pointer, touch};
use smithay::reexports::wayland_server::protocol::wl_pointer::ButtonState;
use smithay::utils::{Logical, Point, Rectangle, Transform, SERIAL_COUNTER};
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::backend::allocator::Fourcc;
use egui::ColorImage;
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;


/**
 * As we currently use Xwayland, there is only 1 surface
 */
fn get_surface(state: &State) -> Option<ToplevelSurface> {
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
                let damage = Rectangle::from_size(size);
                {
                    let (renderer, mut framebuffer) = winit.bind().unwrap();

                    let compositor = &mut backend.compositor;

                    let mut elements: Vec<WindowRenderElement<GlowRenderer>> = Vec::new();

                    compositor.state.terminal_surface_elements.clear();
                    compositor.state.terminal_textures.clear();
                    compositor.state.calculator_surface_elements.clear();
                    compositor.state.calculator_textures.clear();
                    
                    if compositor.state.show_terminal || compositor.state.show_calculator {
                        for surface in compositor.state.xdg_shell_state.toplevel_surfaces() {
                            let elements: Vec<WaylandSurfaceRenderElement<GlowRenderer>> = 
                                render_elements_from_surface_tree(
                                    renderer,
                                    surface.wl_surface(),
                                    (0, 0),
                                    1.0,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            
                            for element in &elements {
                                if let WaylandSurfaceTexture::Texture(texture_id) = element.texture() {
                                    let buffer_size = element.buffer_size();
                                    let region = smithay::utils::Rectangle::from_size(smithay::utils::Size::from((buffer_size.w, buffer_size.h)));
                                    
                                    if let Ok(mapping) = renderer.copy_texture(
                                        texture_id,
                                        region,
                                        Fourcc::Abgr8888,
                                    ) {
                                        if let Ok(pixel_data) = renderer.map_texture(&mapping) {
                                            let color_image = ColorImage::from_rgba_unmultiplied(
                                                [buffer_size.w as usize, buffer_size.h as usize],
                                                pixel_data,
                                            );
                                            
                                            if compositor.state.show_terminal {
                                                let texture_handle = compositor.state.egui_state.context().load_texture(
                                                    format!("terminal_surface_{}", compositor.state.terminal_textures.len()),
                                                    color_image.clone(),
                                                    egui::TextureOptions::default(),
                                                );
                                                compositor.state.terminal_textures.push(texture_handle);
                                            }
                                            
                                            if compositor.state.show_calculator {
                                                let texture_handle = compositor.state.egui_state.context().load_texture(
                                                    format!("calculator_surface_{}", compositor.state.calculator_textures.len()),
                                                    color_image,
                                                    egui::TextureOptions::default(),
                                                );
                                                compositor.state.calculator_textures.push(texture_handle);
                                            }
                                        }
                                    }
                                }
                            }
                            
                            if compositor.state.show_terminal {
                                compositor.state.terminal_surface_elements.extend(elements);
                            } else if compositor.state.show_calculator {
                                compositor.state.calculator_surface_elements.extend(elements);
                            }
                        }
                    }

                    let scale_factor = backend.scale_factor.max(1.0);

                    if let Ok(Some(egui_element)) = compositor.state.egui_state.render(
                        renderer,
                        |ctx| {
                            let mut style = (*ctx.style()).clone();
                            style.text_styles.insert(
                                egui::TextStyle::Body,
                                egui::FontId::new(36.0 * scale_factor as f32, egui::FontFamily::Proportional),
                            );
                            style.text_styles.insert(
                                egui::TextStyle::Button,
                                egui::FontId::new(12.0 * scale_factor as f32, egui::FontFamily::Proportional),
                            );
                            style.text_styles.insert(
                                egui::TextStyle::Heading,
                                egui::FontId::new(48.0 * scale_factor as f32, egui::FontFamily::Proportional),
                            );
                            ctx.set_style(style);
                            
                            egui::CentralPanel::default().show(ctx, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.style_mut().text_styles.insert(
                                            egui::TextStyle::Button,
                                            egui::FontId::new(6.0 * scale_factor as f32, egui::FontFamily::Proportional),
                                        );
                                        
                                        if ui.button(if compositor.state.show_terminal { "Hide Terminal" } else { "Show Terminal" }).clicked() {
                                            log::info!("Terminal button clicked!");
                                            compositor.state.show_terminal = !compositor.state.show_terminal;
                                            
                                            if compositor.state.show_terminal {
                                                compositor.state.show_calculator = false;
                                                if !compositor.state.terminal_spawned {
                                                    std::thread::spawn(|| {
                                                        ArchProcess::exec("WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/tmp weston-terminal").with_log(|log_line| {
                                                            log::info!("Terminal: {}", log_line);
                                                        });
                                                    });
                                                    compositor.state.terminal_spawned = true;
                                                }
                                            } else if !compositor.state.show_terminal && compositor.state.terminal_spawned {
                                                std::thread::spawn(|| {
                                                    ArchProcess::exec("pkill weston-terminal").with_log(|log_line| {
                                                        log::info!("Terminal kill: {}", log_line);
                                                    });
                                                });
                                                compositor.state.terminal_spawned = false;
                                            }
                                        }
                                        
                                        if ui.button(if compositor.state.show_calculator { "Hide Calculator" } else { "Show Calculator" }).clicked() {
                                            log::info!("Calculator button clicked!");
                                            compositor.state.show_calculator = !compositor.state.show_calculator;
                                            
                                            if compositor.state.show_calculator {
                                                compositor.state.show_terminal = false;
                                                if !compositor.state.calculator_spawned {
                                                    std::thread::spawn(|| {
                                                        ArchProcess::exec("WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/tmp QT_SCALE_FACTOR=1.0 QT_AUTO_SCREEN_SCALE_FACTOR=0 QT_FONT_DPI=96 QT_WAYLAND_FORCE_DPI=96 QT_ENABLE_HIGHDPI_SCALING=0 QT_SCREEN_SCALE_FACTORS=1.0 GDK_SCALE=1 GDK_DPI_SCALE=1.0 FONTCONFIG_PATH=/tmp/fontconfig FREETYPE_PROPERTIES=truetype:interpreter-version=40 kcalc").with_log(|log_line| {
                                                            log::info!("Calculator: {}", log_line);
                                                        });
                                                    });
                                                    compositor.state.calculator_spawned = true;
                                                }
                                            } else if !compositor.state.show_calculator && compositor.state.calculator_spawned {
                                                std::thread::spawn(|| {
                                                    ArchProcess::exec("pkill kcalc").with_log(|log_line| {
                                                        log::info!("Calculator kill: {}", log_line);
                                                    });
                                                });
                                                compositor.state.calculator_spawned = false;
                                            }
                                        }
                                    });
                                    
                                    if compositor.state.show_terminal && !compositor.state.show_calculator {
                                        if !compositor.state.terminal_textures.is_empty() {
                                            for (i, texture_handle) in compositor.state.terminal_textures.iter().enumerate() {
                                                let mut size = texture_handle.size_vec2();
                                                size.x = size.x.max(800.0);
                                                size.y = size.y.max(600.0);
                                                ui.image((texture_handle.id(), size));
                                            }
                                        }
                                    }
                                    
                                    if compositor.state.show_calculator && !compositor.state.show_terminal {
                                        if !compositor.state.calculator_textures.is_empty() {
                                            for (i, texture_handle) in compositor.state.calculator_textures.iter().enumerate() {
                                                let mut size = texture_handle.size_vec2();
                                                size.x = size.x.max(600.0);
                                                size.y = size.y.max(700.0);
                                                ui.image((texture_handle.id(), size));
                                            }
                                        }
                                    }
                            });
                        },
                        Rectangle::from_size((size.w, size.h).into()),
                        scale_factor,
                        0.9,
                    ) {
                        elements.push(WindowRenderElement::Egui(egui_element));
                        // log::info!("Egui UI rendered and added to elements with scale {}", scale_factor);
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

                    let terminal_surfaces = compositor.state.xdg_shell_state.toplevel_surfaces();
                    if compositor.state.show_terminal && !terminal_surfaces.is_empty() && !compositor.state.terminal_spawned {
                        compositor.state.terminal_spawned = true;
                    }
                    
                    if compositor.state.show_terminal && !terminal_surfaces.is_empty() {
                        if let Some(surface) = terminal_surfaces.first() {
                            let surface_clone = surface.wl_surface().clone();
                            compositor.keyboard.set_focus(&mut compositor.state, Some(surface_clone), SERIAL_COUNTER.next_serial());
                        }
                    } else {
                        compositor.keyboard.set_focus(&mut compositor.state, None, SERIAL_COUNTER.next_serial());
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
                let should_handle_egui = !state.show_terminal || state.xdg_shell_state.toplevel_surfaces().is_empty();
                let egui_state = state.egui_state.clone();
                let key_pressed = event.state() == smithay::backend::input::KeyState::Pressed;
                
                let serial = SERIAL_COUNTER.next_serial();
                let time = compositor.start_time.elapsed().as_millis() as u32;
                compositor.keyboard.input::<(), _>(
                    state,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
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
                
                let touch_location = (event.x(), event.y()).into();
                compositor.state.egui_state.handle_pointer_motion(touch_location);
                compositor.state.egui_state.handle_pointer_button(
                    smithay::backend::input::MouseButton::Left, 
                    true
                );
                
                if !compositor.state.egui_state.wants_pointer() {
                    let state = &mut compositor.state;
                    if let Some(surface) = get_surface(state) {
                        compositor.keyboard.set_focus(
                            state,
                            Some(surface.wl_surface().clone()),
                            0.into(),
                        );
                        let serial = SERIAL_COUNTER.next_serial();
                        let time = compositor.start_time.elapsed().as_millis() as u32;
                        
                        compositor.touch.down(
                            state,
                            Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                            &touch::DownEvent {
                                slot: event.slot(),
                                location: (event.x(), event.y()).into(),
                                serial,
                                time,
                            },
                        );
                    };
                }
            }
            InputEvent::TouchUp { event } => {
                let compositor = &mut backend.compositor;
                
                compositor.state.egui_state.handle_pointer_button(
                    smithay::backend::input::MouseButton::Left, 
                    false
                );
                
                if !compositor.state.egui_state.wants_pointer() {
                    let state = &mut compositor.state;
                    if let Some(_surface) = get_surface(state) {
                        let serial = SERIAL_COUNTER.next_serial();
                        let time = compositor.start_time.elapsed().as_millis() as u32;
                        
                        compositor.touch.up(
                            state,
                            &touch::UpEvent {
                                slot: event.slot(),
                                serial,
                                time,
                            },
                        );
                    };
                }
            }
            InputEvent::TouchMotion { event } => {
                let compositor = &mut backend.compositor;
                
                let touch_location = (event.x(), event.y()).into();
                compositor.state.egui_state.handle_pointer_motion(touch_location);
                
                if !compositor.state.egui_state.wants_pointer() {
                    let state = &mut compositor.state;
                    if let Some(surface) = get_surface(state) {
                        let time = compositor.start_time.elapsed().as_millis() as u32;
                        
                        compositor.touch.motion(
                            state,
                            Some((surface.wl_surface().clone(), (0f64, 0f64).into())),
                            &touch::MotionEvent {
                                slot: event.slot(),
                                location: (event.x(), event.y()).into(),
                                time,
                            },
                        );
                    };
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let compositor = &mut backend.compositor;
                let pointer = compositor.pointer.clone();
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

                // clamp to screen limits
                pointer_location = clamp_coords(space, pointer_location);

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
            InputEvent::PointerButton { event, .. } => {
                let compositor = &mut backend.compositor;
                
                compositor.state.egui_state.handle_pointer_button(
                    smithay::backend::input::MouseButton::Left, 
                    event.state() == smithay::backend::input::ButtonState::Pressed
                );
                
                if !compositor.state.egui_state.wants_pointer() {
                    let serial = SERIAL_COUNTER.next_serial();
                    let button = event.button_code();
                    let state = ButtonState::from(event.state());
                    let pointer = compositor.pointer.clone();

                    if let Some(surface) = get_surface(&compositor.state) {
                        compositor.keyboard.set_focus(
                            &mut compositor.state,
                            Some(surface.wl_surface().clone()),
                            0.into(),
                        );
                    }
                    pointer.button(
                        &mut compositor.state,
                        &pointer::ButtonEvent {
                            button,
                            state: state.try_into().unwrap(),
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
