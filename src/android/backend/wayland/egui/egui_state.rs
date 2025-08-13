
use egui::{Context, Event, FullOutput, Pos2, RawInput, Rect, Vec2};
use egui::{PlatformOutput, ViewportId, ViewportInfo};
use egui_glow::Painter;
use smithay::{
    backend::{
        allocator::Fourcc,
        input::{Device, DeviceCapability, MouseButton},
        renderer::{
            element::{
                texture::{TextureRenderBuffer, TextureRenderElement},
                Kind,
            },
            gles::{GlesError, GlesTexture},
            glow::GlowRenderer,
            Bind, Frame, Offscreen, Renderer,
        },
    },
    input::{
        keyboard::{KeysymHandle, ModifiersState},
    },
    utils::{IsAlive, Logical, Point, Rectangle, Transform},
};
use xkbcommon::xkb::Keycode;

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};

use super::input;

#[derive(Debug, Clone)]
pub struct EguiState {
    inner: Arc<Mutex<EguiInner>>,
    ctx: Context,
    start_time: Instant,
}

impl PartialEq for EguiState {
    fn eq(&self, other: &Self) -> bool {
        self.ctx == other.ctx
    }
}

struct EguiInner {
    pointers: usize,
    last_pointer_position: Point<i32, Logical>,
    area: Rectangle<i32, Logical>,
    last_modifiers: ModifiersState,
    last_output: Option<PlatformOutput>,
    pressed: Vec<(Option<egui::Key>, Keycode)>,
    focused: bool,
    events: Vec<Event>,
    kbd: Option<input::KbdInternal>,
}

impl fmt::Debug for EguiInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("EguiInner");
        d.field("pointers", &self.pointers)
            .field("last_pointer_position", &self.last_pointer_position)
            .field("area", &self.area)
            .field("last_modifiers", &self.last_modifiers)
            .field("last_output", &self.last_output.as_ref().map(|_| "..."))
            .field("pressed", &self.pressed)
            .field("focused", &self.focused)
            .field("events", &self.events)
            .field("kbd", &self.kbd);
        d.finish()
    }
}

struct GlState {
    painter: Painter,
    render_buffers: HashMap<usize, TextureRenderBuffer<GlesTexture>>,
}

type UserDataType = Rc<RefCell<GlState>>;

impl EguiState {
    /// Creates a new `EguiState`
    pub fn new(area: Rectangle<i32, Logical>) -> EguiState {
        let ctx = Context::default();
        
        ctx.set_pixels_per_point(1.0); // Reduced DPI for better button sizing
        EguiState {
            ctx,
            start_time: Instant::now(),
            inner: Arc::new(Mutex::new(EguiInner {
                pointers: 0,
                last_pointer_position: (0, 0).into(),
                area,
                last_modifiers: ModifiersState::default(),
                last_output: None,
                events: Vec::new(),
                focused: false,
                pressed: Vec::new(),
                kbd: match input::KbdInternal::new() {
                    Some(kbd) => Some(kbd),
                    None => {
                        log::error!("Failed to initialize keymap for text input in egui.");
                        None
                    }
                },
            })),
        }
    }

    fn id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    /// Retrieve the underlying [`egui::Context`]
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// If true, egui is currently listening on text input (e.g. typing text in a TextEdit).
    pub fn wants_keyboard(&self) -> bool {
        self.ctx.wants_keyboard_input()
    }

    /// True if egui is currently interested in the pointer (mouse or touch).
    pub fn wants_pointer(&self) -> bool {
        self.ctx.wants_pointer_input()
    }

    /// Pass new input devices to `EguiState` for internal tracking
    pub fn handle_device_added(&self, device: &impl Device) {
        if device.has_capability(DeviceCapability::Pointer) {
            self.inner.lock().unwrap().pointers += 1;
        }
    }

    /// Remove input devices to `EguiState` for internal tracking
    pub fn handle_device_removed(&self, device: &impl Device) {
        let mut inner = self.inner.lock().unwrap();
        if device.has_capability(DeviceCapability::Pointer) {
            inner.pointers -= 1;
        }
        if inner.pointers == 0 {
            inner.events.push(Event::PointerGone);
        }
    }

    /// Pass keyboard events into `EguiState`.
    pub fn handle_keyboard(&self, handle: &KeysymHandle, pressed: bool, modifiers: ModifiersState) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_modifiers = modifiers;
        let key = if let Some(key) = input::convert_key(handle.raw_syms().iter().copied()) {
            inner.events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: input::convert_modifiers(modifiers),
            });
            Some(key)
        } else {
            None
        };

        if pressed {
            inner.pressed.push((key, handle.raw_code()));
        } else {
            inner.pressed.retain(|(_, code)| code != &handle.raw_code());
        }

        if let Some(kbd) = &mut inner.kbd {
            kbd.key_input(handle.raw_code().raw(), pressed);
            if pressed {
                let text = kbd.get_utf8(handle.raw_code().raw());
                if !text.is_empty() && text != "\x7f" && text != "\x08" {
                    inner.events.push(Event::Text(text));
                }
            }
        }
    }

    pub fn handle_pointer_motion(&self, location: Point<f64, Logical>) {
        let mut inner = self.inner.lock().unwrap();
        let pos = Pos2::new(location.x as f32, location.y as f32);
        inner.last_pointer_position = (location.x as i32, location.y as i32).into();
        inner.events.push(Event::PointerMoved(pos));
    }

    pub fn handle_pointer_button(&self, button: MouseButton, pressed: bool) {
        if let Some(button) = input::convert_button(button) {
            let mut inner = self.inner.lock().unwrap();
            let pos = Pos2::new(
                inner.last_pointer_position.x as f32,
                inner.last_pointer_position.y as f32,
            );
            let modifiers = input::convert_modifiers(inner.last_modifiers);
            inner.events.push(Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            });
        }
    }

    pub fn handle_pointer_axis(&self, horizontal: f64, vertical: f64) {
        let mut inner = self.inner.lock().unwrap();
        let delta = Vec2::new(horizontal as f32, vertical as f32);
        let modifiers = input::convert_modifiers(inner.last_modifiers);
        inner.events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta,
            modifiers,
        });
    }

    pub fn set_focused(&self, focused: bool) {
        self.inner.lock().unwrap().focused = focused;
    }

    /// Produce a new frame of egui. Returns a TextureRenderElement for rendering
    pub fn render<F>(
        &self,
        renderer: &mut GlowRenderer,
        mut ui: F,
        area: Rectangle<i32, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Result<Option<TextureRenderElement<GlesTexture>>, GlesError>
    where
        F: FnMut(&Context),
    {
        let int_scale = scale.ceil() as i32;
        
        let needs_painter_init = {
            let user_data = renderer.egl_context().user_data();
            user_data.get::<UserDataType>().is_none()
        };
        
        if needs_painter_init {
            let painter = renderer.with_context(|context| {
                Painter::new(context.clone(), "", None, false).unwrap()
            })?;
            
            let user_data = renderer.egl_context().user_data();
            user_data.insert_if_missing(|| {
                UserDataType::new(RefCell::new(GlState {
                    painter,
                    render_buffers: HashMap::new(),
                }))
            });
        }

        let mut inner = self.inner.lock().unwrap();
        let gl_state = renderer
            .egl_context()
            .user_data()
            .get::<UserDataType>()
            .unwrap()
            .clone();
        let mut borrow = gl_state.borrow_mut();
        let GlState {
            ref mut painter,
            ref mut render_buffers,
            ..
        } = &mut *borrow;

        let render_buffer = render_buffers.entry(self.id()).or_insert_with(|| {
            let render_texture = renderer
                .create_buffer(
                    Fourcc::Abgr8888,
                    (area.size.w * int_scale, area.size.h * int_scale).into(),
                )
                .expect("Failed to create buffer");
            TextureRenderBuffer::from_texture(
                renderer,
                render_texture,
                int_scale,
                Transform::Flipped180,
                None,
            )
        });

        let size = area.size;
        let input = RawInput {
            viewport_id: ViewportId::ROOT,
            viewports: std::iter::once((
                ViewportId::ROOT,
                ViewportInfo {
                    native_pixels_per_point: Some(scale as f32),
                    ..Default::default()
                },
            ))
            .collect(),
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(size.w as f32, size.h as f32),
            )),
            max_texture_side: Some(painter.max_texture_side()),
            time: Some(self.start_time.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: input::convert_modifiers(inner.last_modifiers),
            events: inner.events.drain(..).collect(),
            hovered_files: Vec::new(),
            dropped_files: Vec::new(),
            focused: inner.focused,
            system_theme: None,
        };

        let FullOutput {
            platform_output,
            shapes,
            textures_delta,
            ..
        } = self.ctx.run(input, &mut ui);
        inner.last_output = Some(platform_output);

        if shapes.is_empty() {
            return Ok(None);
        }

        let needs_recreate = inner.area != area;
        inner.area = area;

        if needs_recreate {
            *render_buffer = {
                let render_texture = renderer.create_buffer(
                    Fourcc::Abgr8888,
                    (area.size.w * int_scale, area.size.h * int_scale).into(),
                )?;
                TextureRenderBuffer::from_texture(
                    renderer,
                    render_texture,
                    int_scale,
                    Transform::Flipped180,
                    None,
                )
            };
        }

        render_buffer.render().draw(|tex| {
            let mut fb = renderer.bind(tex)?;
            let physical_area = area.to_physical(int_scale);
            {
                let mut frame = renderer.render(&mut fb, physical_area.size, Transform::Normal)?;
                frame.clear([0.0, 0.0, 0.0, 0.0].into(), &[physical_area])?;
                
                painter.paint_and_update_textures(
                    [physical_area.size.w as u32, physical_area.size.h as u32],
                    scale as f32,
                    &self.ctx.tessellate(shapes, scale as f32),
                    &textures_delta,
                );
            }
            Result::<_, GlesError>::Ok(vec![Rectangle::from_loc_and_size(
                (0, 0),
                (physical_area.size.w, physical_area.size.h)
            )])
        })?;

        Ok(Some(TextureRenderElement::from_texture_render_buffer(
            area.loc.to_f64().to_physical(scale),
            &render_buffer,
            Some(alpha),
            None,
            None,
            Kind::Unspecified,
        )))
    }

    pub fn last_output(&self) -> Option<PlatformOutput> {
        self.inner.lock().unwrap().last_output.clone()
    }
}

impl IsAlive for EguiState {
    fn alive(&self) -> bool {
        true
    }
}
