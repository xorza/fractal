#![feature(portable_simd)]
#![allow(dead_code)]

use std::sync::Arc;

use bytemuck::Zeroable;
use pollster::FutureExt;
use tokio::time::Instant;
use wgpu::Limits;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::event::{ElementState, Event, EventResult, MouseButtons};
use crate::math::{Vec2i32, Vec2u32};
use crate::tiled_fractal_app::UserEvent;

mod event;
mod math;
mod render_pods;
mod mandel_texture;
mod tiled_fractal_app;
mod env;
mod mandelbrot_simd;

type UserEventType = UserEvent;

struct WindowContext<'window> {
    window: Arc<winit::window::Window>,
    surface: wgpu::Surface<'window>,
    surface_config: wgpu::SurfaceConfiguration,

    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,

}

struct AppState<'window> {
    window: Option<WindowContext<'window>>,
    fractal_app: Option<tiled_fractal_app::TiledFractalApp>,

    event_loop_proxy: EventLoopProxy<UserEventType>,

    start: Instant,

    is_redrawing: bool,
    is_resizing: bool,
    has_render_error_scope: bool,
    mouse_position: Option<Vec2u32>,
}

pub struct RenderContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub view: &'a wgpu::TextureView,
    pub time: f64,
}

fn main() {
    let event_loop: EventLoop<UserEventType> = EventLoop::<UserEventType>::with_user_event()
        .build()
        .unwrap();
    let mut app_state = AppState {
        window: None,
        fractal_app: None,
        is_redrawing: false,
        is_resizing: false,
        has_render_error_scope: false,
        start: Instant::now(),
        mouse_position: None,
        event_loop_proxy: event_loop.create_proxy(),
    };
    event_loop.run_app(&mut app_state).unwrap();
}

impl<'a> ApplicationHandler<UserEventType> for AppState<'_> {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        let _ = (event_loop, cause);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window_attr = winit::window::Window::default_attributes()
            .with_title("Mandelbrot explorer");
        let window = event_loop.create_window(window_attr).unwrap();
        let window = Arc::new(window);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: Default::default(),
            dx12_shader_compiler: wgpu::Dx12Compiler::Dxc { dxil_path: None, dxc_path: None },
            gles_minor_version: Default::default(),
        });
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .block_on()
            .expect("No suitable GPU adapters found on the system.");

        // Make sure we use the texture resolution limits from the adapter, so we can support images the size of the surface.
        let limits = Limits {
            max_push_constant_size: 1024,
            ..Default::default()
        }.using_resolution(adapter.limits());

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::PUSH_CONSTANTS,
                    required_limits: limits,
                },
                None,
            )
            .block_on()
            .expect("Unable to find a suitable GPU adapter.");

        let window_size = window.inner_size();
        let mut surface_config = surface
            .get_default_config(&adapter, window_size.width, window_size.height)
            .expect("Surface isn't supported by the adapter.");
        let surface_view_format = surface_config.format.add_srgb_suffix();
        surface_config.view_formats.push(surface_view_format);
        surface.configure(&device, &surface_config);


        self.window = Some(WindowContext {
            window: window.clone(),
            surface,
            surface_config,
            adapter,
            device,
            queue,
        });
        let window_state = self.window.as_ref().unwrap();

        self.fractal_app = Some(tiled_fractal_app::TiledFractalApp::new(
            window_state,
            self.event_loop_proxy.clone(),
        ));

        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEventType) {
        if self.window.is_none() {
            return;
        }
        
        let result = self.fractal_app.as_mut().unwrap().update(Event::Custom(event));
        self.process_event_result(event_loop, result);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: winit::event::WindowEvent) {
        if self.window.is_none() {
            return;
        }
        
        if self.mouse_position.is_none() {
            match event {
                winit::event::WindowEvent::CursorMoved { position, .. } => {
                    let position = Vec2u32::new(position.x as u32, position.y as u32);
                    self.mouse_position = Some(position);
                }
                _ => {}
            }
        }

        let result: EventResult =
            match event {
                winit::event::WindowEvent::Resized(_) | winit::event::WindowEvent::ScaleFactorChanged { .. } => {
                    let window_state = self.window.as_mut().unwrap();
                    let window_size = window_state.window.inner_size();

                    let window_size = Vec2u32::new(window_size.width.max(1), window_size.height.max(1));
                    window_state.surface_config.width = window_size.x;
                    window_state.surface_config.height = window_size.y;
                    window_state.surface.configure(&window_state.device, &window_state.surface_config);

                    self.fractal_app.as_mut().unwrap().update(Event::Resized(window_size))
                }
                winit::event::WindowEvent::RedrawRequested => {
                    let window_state = self.window.as_mut().unwrap();

                    self.is_redrawing = true;

                    let surface_texture = match window_state.surface.get_current_texture() {
                        Ok(frame) => frame,
                        Err(_) => {
                            window_state.surface.configure(&window_state.device, &window_state.surface_config);
                            window_state.surface
                                .get_current_texture()
                                .expect("Failed to acquire next surface texture.")
                        }
                    };
                    let surface_texture_view = surface_texture.texture.create_view(
                        &wgpu::TextureViewDescriptor {
                            format: Some(window_state.surface_config.format),
                            ..wgpu::TextureViewDescriptor::default()
                        });

                    assert!(!self.has_render_error_scope);
                    window_state.device.push_error_scope(wgpu::ErrorFilter::Validation);
                    self.has_render_error_scope = true;

                    self.fractal_app.as_mut().unwrap().render(&RenderContext {
                        device: &window_state.device,
                        queue: &window_state.queue,
                        view: &surface_texture_view,
                        time: self.start.elapsed().as_secs_f64(),
                    });

                    surface_texture.present();

                    EventResult::Continue
                }

                event => {
                    let mut empty_mouse_position = Vec2u32::zeroed();
                    let mouse_position = self.mouse_position.as_mut().unwrap_or(&mut empty_mouse_position);
                    let event = process_window_event(event, mouse_position);

                    self.fractal_app.as_mut().unwrap().update(event)
                }
            };

        self.process_event_result(event_loop, result);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, device_id: DeviceId, event: DeviceEvent) {
        let _ = (event_loop, device_id, event);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            return;
        }
        
        let mut results: [EventResult; 2] = [
            EventResult::Continue,
            EventResult::Continue,
        ];

        if self.is_redrawing {
            self.is_redrawing = false;
            if self.has_render_error_scope {
                self.has_render_error_scope = false;

                let window_state = self.window.as_ref().unwrap();
                if let Some(error) = window_state.device.pop_error_scope().block_on() {
                    panic!("Device error: {:?}", error);
                }
            }

            results[0] = self.fractal_app.as_mut().unwrap().update(Event::RedrawFinished);
        }

        if self.is_resizing {
            self.is_resizing = false;

            let window_size = self.window.as_ref().unwrap().window.inner_size();

            results[1] = self.fractal_app.as_mut().unwrap().update(
                Event::Resized(Vec2u32::new(window_size.width, window_size.height))
            );
        }
        
        if results.iter().any(|result| matches!(result, EventResult::Exit)) {
            _event_loop.exit();
        } else if results.iter().any(|result| matches!(result, EventResult::Redraw)) {
            self.window.as_ref().unwrap().window.request_redraw();
        }
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
        self.window = None; 
        self.fractal_app = None;
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
    }
}

impl<'a> AppState<'_> {
    fn process_event_result(&mut self, event_loop: &ActiveEventLoop, event_result: EventResult) {
        match event_result {
            EventResult::Continue => {}

            EventResult::Redraw => {
                self.window.as_ref().unwrap().window.request_redraw();
            }
            EventResult::Exit => {
                event_loop.exit();
            }
        }
    }
}


fn process_window_event<UserEvent>(event: winit::event::WindowEvent, mouse_position: &mut Vec2u32) -> Event<UserEvent> {
    match event {
        winit::event::WindowEvent::Resized(size) =>
            Event::Resized(
                Vec2u32::new(size.width.max(1), size.height.max(1)),
            ),
        winit::event::WindowEvent::Focused(_is_focused) => {
            Event::Unknown
        }
        winit::event::WindowEvent::CursorEntered { .. } => {
            Event::Unknown
        }
        winit::event::WindowEvent::CursorLeft { .. } => {
            Event::Unknown
        }
        winit::event::WindowEvent::CursorMoved { position: _position, .. } => {
            let prev_pos = *mouse_position;
            let new_pos = Vec2u32::new(_position.x as u32, _position.y as u32);
            *mouse_position = new_pos;

            Event::MouseMove {
                position: new_pos,
                delta: Vec2i32::from(new_pos) - Vec2i32::from(prev_pos),
            }
        }
        winit::event::WindowEvent::Occluded(_is_occluded) => {
            Event::Unknown
        }
        winit::event::WindowEvent::MouseInput { state, button, .. } => {
            Event::MouseButton(
                MouseButtons::from(button),
                ElementState::from(state),
                mouse_position.clone(),
            )
        }
        winit::event::WindowEvent::MouseWheel { delta, phase: _phase, .. } => {
            match delta {
                winit::event::MouseScrollDelta::LineDelta(_l1, l2) => {
                    Event::MouseWheel(mouse_position.clone(), l2)
                }
                winit::event::MouseScrollDelta::PixelDelta(_pix) => {
                    Event::Unknown
                }
            }
        }
        winit::event::WindowEvent::PinchGesture { device_id: _device_id, delta, phase: _phase } => {
            // Event::TouchpadMagnify(mouse_position.clone(), delta as f32)
            Event::MouseWheel(mouse_position.clone(), -50.0 * delta as f32)
        }
        winit::event::WindowEvent::CloseRequested => {
            Event::WindowClose
        }
        winit::event::WindowEvent::Moved(_position) => {
            Event::Unknown
        }
        _ => Event::Unknown,
    }
}
