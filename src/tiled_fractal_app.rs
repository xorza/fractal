#![allow(unused_parens)]

use std::sync::{Arc, Mutex};

use bytemuck::Zeroable;
use tokio::runtime::Runtime;
use winit::event_loop::EventLoopProxy;

use crate::event::{ElementState, Event, EventResult, MouseButtons};
use crate::mandel_texture::MandelTexture;
use crate::math::{RectF64, Vec2f64, Vec2i32, Vec2u32};
use crate::{RenderContext, WindowContext};

enum ManipulateState {
    Idle,
    Drag,
}

pub struct TiledFractalApp {
    window_size: Vec2u32,
    event_loop_proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    runtime: Runtime,

    manipulate_state: ManipulateState,

    frame_rect: RectF64,
    aspect: Vec2f64,

    mandel_texture: MandelTexture,
}


#[derive(Debug)]
pub enum UserEvent {
    Redraw,
    TileReady {
        tile_index: usize,
    },
}

impl TiledFractalApp {
    pub fn new(
        window_state: &WindowContext,
        event_loop_proxy: EventLoopProxy<UserEvent>,
    ) -> TiledFractalApp {
        let window_size = Vec2u32::new(window_state.surface_config.width, window_state.surface_config.height);

        let mandel_texture = MandelTexture::new(
            &window_state.device,
            &window_state.queue,
            &window_state.surface_config,
            window_size,
        );

        let aspect = Vec2f64::new(window_size.x as f64 / window_size.y as f64, 1.0);
        let frame_rect = RectF64::from_center_size(
            Vec2f64::zeroed(),
            aspect * 2.5,
        );

        let mut result = Self {
            window_size,
            event_loop_proxy: Arc::new(Mutex::new(event_loop_proxy)),
            runtime: Runtime::new().unwrap(),

            manipulate_state: ManipulateState::Idle,

            frame_rect,
            aspect,

            mandel_texture,
        };
        result.update_fractal(result.frame_rect.center());
        return result;
    }

    pub fn update(&mut self, event: Event<UserEvent>) -> EventResult {
        match event {
            Event::WindowClose => EventResult::Exit,
            Event::Resized(window_size) => {
                if self.window_size == window_size {
                    return EventResult::Continue;
                }

                self.frame_rect = RectF64::from_center_size(
                    self.frame_rect.center(),
                    self.frame_rect.size * Vec2f64::from(window_size) / Vec2f64::from(self.window_size),
                );
                self.window_size = window_size;
                self.mandel_texture.resize_window(window_size);

                self.update_fractal(self.frame_rect.center());

                EventResult::Redraw
            }

            Event::MouseWheel(position, delta) => {
                self.move_scale(position, Vec2i32::zeroed(), delta);

                EventResult::Redraw
            }
            Event::MouseMove { position, delta } => {
                match self.manipulate_state {
                    ManipulateState::Idle => EventResult::Continue,
                    ManipulateState::Drag => {
                        self.move_scale(position, delta, 0.0);

                        EventResult::Redraw
                    }
                }
            }
            Event::MouseButton(btn, state, _position) => {
                match (btn, state) {
                    (MouseButtons::Left, ElementState::Pressed) => {
                        self.manipulate_state = ManipulateState::Drag;
                        EventResult::Continue
                    }
                    _ => {
                        self.manipulate_state = ManipulateState::Idle;
                        EventResult::Continue
                    }
                }
            }

            Event::Custom(event) => {
                self.update_user_event(event)
            }

            _ => EventResult::Continue
        }
    }

    pub fn render(&mut self, render_info: &RenderContext) {
        self.mandel_texture.render(render_info);
    }


    fn move_scale(&mut self, mouse_pos: Vec2u32, mouse_delta: Vec2i32, scroll_delta: f32) {
        let mouse_pos = Vec2i32::new(mouse_pos.x as i32, self.window_size.y as i32 - mouse_pos.y as i32);
        let mouse_pos = Vec2f64::from(mouse_pos) / Vec2f64::from(self.window_size);
        let mouse_pos = mouse_pos - 0.5f64;

        let mouse_delta = Vec2f64::from(mouse_delta) / Vec2f64::from(self.window_size);
        let mouse_delta = Vec2f64::new(mouse_delta.x, -mouse_delta.y);

        let zoom = 1.15f64.powf(scroll_delta as f64 / 5.0f64);

        let old_size = self.frame_rect.size;
        let new_size = old_size * zoom;

        let old_offset = self.frame_rect.center();
        let new_offset =
            old_offset
                - mouse_delta * new_size
                - mouse_pos * (new_size - old_size);

        self.frame_rect = RectF64::from_center_size(
            new_offset,
            new_size,
        );

        let focus = self.frame_rect.center()
            + self.frame_rect.size * mouse_pos;

        self.update_fractal(focus);
    }

    fn update_user_event(&mut self, event: UserEvent) -> EventResult {
        match event {
            UserEvent::Redraw => EventResult::Redraw,
            UserEvent::TileReady { tile_index: _tile_index } => {
                EventResult::Redraw
            }
        }
    }

    fn update_fractal(&mut self, focus: Vec2f64) {
        let event_loop_proxy = self.event_loop_proxy.clone();

        self.mandel_texture.update(
            self.frame_rect,
            focus,
            move |index| {
                event_loop_proxy.lock().unwrap().send_event(
                    UserEvent::TileReady {
                        tile_index: index,
                    }
                ).unwrap();
            },
        );
    }
}

