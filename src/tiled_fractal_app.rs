#![allow(unused_parens)]

use std::mem::swap;
use std::sync::{Arc, Mutex};

use bytemuck::Zeroable;
use tokio::runtime::Runtime;
use wgpu::TextureAspect;
use winit::event_loop::EventLoopProxy;

use crate::app_base::{App, RenderInfo};
use crate::event::{ElementState, Event, EventResult, MouseButtons};
use crate::mandel_texture::{MandelTexture, TileState};
use crate::math::{Vec2f32, Vec2f64, Vec2i32, Vec2u32};
use crate::wgpu_renderer::{ScreenTexBindGroup, WgpuRenderer};

enum ManipulateState {
    Idle,
    Drag,
}

pub struct TiledFractalApp {
    window_size: Vec2u32,
    renderer: WgpuRenderer,
    event_loop: EventLoopProxy<UserEvent>,
    runtime: Runtime,

    manipulate_state: ManipulateState,

    final_offset: Vec2f64,
    final_scale: f64,

    draft_offset: Vec2f32,
    draft_scale: f32,

    mandel_texture: MandelTexture,
    screen_tex_bind_group: ScreenTexBindGroup,

    has_update_tiles: bool,
}

#[derive(Debug)]
pub enum UserEvent {
    Redraw,
    TileReady {
        tile_index: usize,
    },
}

impl App for TiledFractalApp {
    type UserEventType = UserEvent;

    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_config: &wgpu::SurfaceConfiguration,
        event_loop_proxy: EventLoopProxy<UserEvent>,
    ) -> TiledFractalApp
    {
        let window_size = Vec2u32::new(surface_config.width, surface_config.height);
        let renderer = WgpuRenderer::new(device, queue, surface_config, window_size);

        let mandel_texture = MandelTexture::new(device);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &renderer.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&mandel_texture.tex_view),
                },
            ],
            label: None,
        });

        let screen_tex_bind_group = ScreenTexBindGroup {
            bind_group,
            texture_size: mandel_texture.size,
        };

        Self {
            window_size,
            renderer,
            event_loop: event_loop_proxy,
            runtime: Runtime::new().unwrap(),

            manipulate_state: ManipulateState::Idle,

            final_offset: Vec2f64::zeroed(),
            final_scale: 1.0f64,

            draft_offset: Vec2f32::zeroed(),
            draft_scale: 1.0f32,

            mandel_texture,
            screen_tex_bind_group,

            has_update_tiles: false,
        }
    }

    fn update(&mut self, event: Event<UserEvent>) -> EventResult {
        let result = match event {
            Event::WindowClose => EventResult::Exit,
            Event::Resized(_size) => EventResult::Redraw,

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

            Event::Init => {
                self.render_fractal();

                EventResult::Continue
            }

            _ => EventResult::Continue
        };

        result
    }

    fn render(&mut self, render_info: RenderInfo) {
        if self.has_update_tiles {
            self.has_update_tiles = false;
            self.update_tiles(&render_info);
        }

        self.renderer.go(
            &render_info,
            &self.screen_tex_bind_group,
        );
    }

    fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, window_size: Vec2u32) {
        if self.window_size == window_size {
            return;
        }

        self.window_size = window_size;
        self.renderer.resize(device, queue, window_size);
    }
}

impl TiledFractalApp {
    fn move_scale(&mut self, mouse_pos: Vec2u32, mouse_delta: Vec2i32, scroll_delta: f32) {
        let mouse_pos = Vec2f32::from(mouse_pos)
            / Vec2f32::from(self.window_size);
        let mouse_pos = Vec2f32::new(mouse_pos.x, 1.0 - mouse_pos.y);

        let mouse_delta = Vec2f32::from(mouse_delta)
            / Vec2f32::from(self.window_size);
        let mouse_delta = Vec2f32::new(mouse_delta.x, -mouse_delta.y);

        let zoom = 1.15f32.powf(scroll_delta / 5.0);

        {
            let old_final_scale = self.final_scale;
            let new_final_scale = old_final_scale / zoom as f64;

            let old_offset = self.final_offset;
            let new_offset =
                Vec2f64::from(mouse_delta) * new_final_scale
                    + old_offset
                    + (Vec2f64::from(mouse_pos) - 0.5) * (new_final_scale - old_final_scale);

            self.final_scale = new_final_scale;
            self.final_offset = new_offset;
        }

        {
            let mouse_pos = mouse_pos * 2.0f32 - 1.0f32;

            let old_draft_scale = self.draft_scale;
            let new_draft_scale = old_draft_scale * zoom;

            let old_draft_offset = self.draft_offset;
            let new_draft_offset =
                2.0 * mouse_delta * new_draft_scale
                    + old_draft_offset
                    - mouse_pos * (new_draft_scale - old_draft_scale);

            self.draft_scale = new_draft_scale;
            self.draft_offset = new_draft_offset;
        }
    }

    fn update_user_event(&mut self, event: UserEvent) -> EventResult {
        match event {
            UserEvent::Redraw => EventResult::Redraw,
            UserEvent::TileReady { tile_index: _tile_index } => {
                self.has_update_tiles = true;
                EventResult::Redraw
            }
            // _ => EventResult::Continue
        }
    }

    fn render_fractal(&mut self) {
        let event_loop_proxy =
            Arc::new(Mutex::new(self.event_loop.clone()));

        self.mandel_texture.render(
            &self.runtime,
            move |index| {
                event_loop_proxy.lock().unwrap().send_event(
                    UserEvent::TileReady {
                        tile_index: index,
                    }
                ).unwrap();
            },
        );
    }
    fn update_tiles(&self, render_info: &RenderInfo) {
        self.mandel_texture.tiles
            .iter()
            .for_each(|tile| {
                let mut buff: Option<Vec<u8>> = None;

                {
                    let mut tile_state = tile.state.lock().unwrap();
                    if let TileState::Ready { buffer } = &mut *tile_state {
                        let mut new_buff: Vec<u8> = Vec::new();
                        swap(&mut new_buff, buffer);
                        buff = Some(new_buff);
                    }
                    if matches!(*tile_state, TileState::Ready { .. }) {
                        *tile_state = TileState::Idle;
                    }
                }

                if let Some(buff) = buff {
                    render_info.queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &self.mandel_texture.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: tile.offset.x,
                                y: tile.offset.y,
                                z: 0,
                            },
                            aspect: TextureAspect::All,
                        },
                        &buff,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row:  Some(tile.size.x),
                            rows_per_image: Some(tile.size.y),
                        },
                        wgpu::Extent3d {
                            width:  tile.size.x,
                            height: tile.size.y,
                            depth_or_array_layers: 1,
                        },
                    );
                    println!("tile updated");
                }
            });
    }
}
