use std::mem::swap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use bytemuck::Zeroable;
use num_complex::Complex;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::app_base::RenderInfo;
use crate::math::{RectF64, RectU32, Vec2f64, Vec2u32};

const TILE_SIZE: u32 = 128;

pub enum TileState {
    Idle,
    Computing {
        task_handle: JoinHandle<()>,
    },
    WaitForUpload {
        buffer: Vec<u8>,
    },
    Ready,
}

pub struct Tile {
    pub index: usize,
    pub tex_rect: RectU32,
    pub state: Arc<Mutex<TileState>>,
    pub cancel_token: Arc<AtomicU32>,
}

pub struct MandelTexture {
    pub texture1: wgpu::Texture,
    pub texture_view1: wgpu::TextureView,

    window_size: Vec2u32,

    runtime: Runtime,

    pub tex_size: Vec2u32,
    pub max_iter: u32,
    pub tiles: Vec<Tile>,
    pub fractal_rect: RectF64,
}


impl MandelTexture {
    pub fn new(
        device: &wgpu::Device,
        window_size: Vec2u32,
    ) -> Self {
        let tex_size =
            1024 * 2
            // device.limits().max_texture_dimension_2d
            ;
        assert!(tex_size >= 1024);

        let texture_extent = wgpu::Extent3d {
            width: tex_size,
            height: tex_size,
            depth_or_array_layers: 1,
        };
        let texture1 = device.create_texture(&wgpu::TextureDescriptor {
            size: texture_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
            label: None,
        });
        let texture_view1 = texture1.create_view(&wgpu::TextureViewDescriptor::default());

        assert_eq!(tex_size % TILE_SIZE, 0);
        let tile_count = tex_size / TILE_SIZE;
        let mut tiles = Vec::with_capacity(tile_count as usize * tile_count as usize);
        for i in 0..tile_count {
            for j in 0..tile_count {
                let index = tiles.len();
                let rect = RectU32 {
                    pos: Vec2u32::new(i * TILE_SIZE, j * TILE_SIZE),
                    size: Vec2u32::new(TILE_SIZE, TILE_SIZE),
                };
                tiles.push(Tile {
                    index,
                    tex_rect: rect,
                    state: Arc::new(Mutex::new(TileState::Idle)),
                    cancel_token: Arc::new(AtomicU32::new(0)),
                });
            }
        }

        let runtime = Runtime::new().unwrap();


        Self {
            texture1,
            texture_view1,

            window_size,

            runtime,

            tex_size: Vec2u32::all(tex_size),
            max_iter: 100,
            tiles,

            fractal_rect: RectF64::zeroed(),
        }
    }

    pub fn update<F>(
        &mut self,
        frame_rect: RectF64,
        tile_ready_callback: F,
    )
    where F: Fn(usize) + Clone + Send + Sync + 'static
    {
        // let focus = frame_rect.center();
        // self.tiles.sort_unstable_by(|a, b| {
        //     let a_center = Vec2f64::from(a.tex_rect.center());
        //     let b_center = Vec2f64::from(b.tex_rect.center());
        //
        //     let a_dist = (a_center - focus).length_squared();
        //     let b_dist = (b_center - focus).length_squared();
        //
        //     a_dist.partial_cmp(&b_dist).unwrap()
        // });

        let a = self.tex_size.x as f64 / self.window_size.x as f64;
        let b = self.fractal_rect.size.x / frame_rect.size.x;
        let scale_changed =
            (a - b).abs() > f64::EPSILON
            ;
        if scale_changed {
            self.fractal_rect = RectF64::center_size(
                frame_rect.center(),
                Vec2f64::all(a * frame_rect.size.x),
            );
            // println!("frame_rect:   {:?}, center: {:?}", frame_rect, frame_rect.center());
            // println!("fractal_rect: {:?}, center: {:?}", self.fractal_rect, self.fractal_rect.center());
        }


        let fractal_rect = self.fractal_rect;

        self.tiles
            .iter()
            .for_each(|tile| {
                let mut tile_state_mutex = tile.state.lock().unwrap();
                let tile_state = &mut *tile_state_mutex;

                if scale_changed {
                    tile.cancel_token.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if let TileState::Computing { task_handle } = tile_state {
                        task_handle.abort();
                    }
                    *tile_state = TileState::Idle;
                }

                let tile_rect = tile.fractal_rect(
                    self.tex_size,
                    self.fractal_rect,
                );
                if !frame_rect.intersects(&tile_rect) {
                    if let TileState::Computing { task_handle } = tile_state {
                        tile.cancel_token.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        task_handle.abort();
                        *tile_state = TileState::Idle;
                    }
                    return;
                }

                if !matches!(tile_state, TileState::Idle) {
                    return;
                }

                let img_size = self.tex_size;
                let tile_rect = tile.tex_rect;
                let tile_index = tile.index;

                let callback = tile_ready_callback.clone();
                let cancel_token = tile.cancel_token.clone();
                let tile_state_clone = tile.state.clone();

                let task_handle = self.runtime.spawn(async move {
                    let buf = mandelbrot(
                        img_size,
                        tile_rect,
                        -fractal_rect.center(),
                        1.0 / fractal_rect.size.y,
                        cancel_token,
                    )
                        .await
                        .ok();

                    let mut tile_state = tile_state_clone.lock().unwrap();
                    if let Some(buf) = buf {
                        *tile_state = TileState::WaitForUpload {
                            buffer: buf,
                        };
                        (callback)(tile_index);
                    } else {
                        *tile_state = TileState::Idle;
                    }
                });

                *tile_state = TileState::Computing {
                    task_handle,
                };
            });
    }

    pub fn render(&self, render_info: &RenderInfo) {
        self.tiles
            .iter()
            .for_each(|tile| {
                let mut buff: Option<Vec<u8>> = None;

                {
                    let mut tile_state = tile.state.lock().unwrap();
                    if let TileState::WaitForUpload { buffer } = &mut *tile_state {
                        let mut new_buff: Vec<u8> = Vec::new();
                        swap(&mut new_buff, buffer);
                        buff = Some(new_buff);
                    }
                    if buff.is_some() {
                        *tile_state = TileState::Ready;
                    } else {
                        return;
                    }
                }

                let buff = buff.unwrap();
                render_info.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: &self.texture1,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: tile.tex_rect.pos.x,
                            y: tile.tex_rect.pos.y,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &buff,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(tile.tex_rect.size.x),
                        rows_per_image: Some(tile.tex_rect.size.y),
                    },
                    wgpu::Extent3d {
                        width: tile.tex_rect.size.x,
                        height: tile.tex_rect.size.y,
                        depth_or_array_layers: 1,
                    },
                );
            });
    }

    pub fn resize_window(&mut self, window_size: Vec2u32) {
        self.window_size = window_size;
    }
}

impl Tile {
    pub(crate) fn fractal_rect(&self, tex_size: Vec2u32, fractal_rect: RectF64) -> RectF64 {
        let abs_frame_size = Vec2f64::from(tex_size);
        let abs_tile_pos = Vec2f64::from(self.tex_rect.pos);
        let abs_tile_size = Vec2f64::from(self.tex_rect.size);

        let tile_size =
            fractal_rect.size * abs_tile_size / abs_frame_size;
        let tile_pos =
            fractal_rect.pos + fractal_rect.size * abs_tile_pos / abs_frame_size;


        RectF64::pos_size(tile_pos, tile_size)
    }
}

//noinspection RsConstantConditionIf
async fn mandelbrot(
    img_size: Vec2u32,
    tile_rect: RectU32,
    fractal_offset: Vec2f64,
    fractal_scale: f64,
    cancel_token: Arc<AtomicU32>,
) -> anyhow::Result<Vec<u8>>
{
    let cancel_token_value = cancel_token.load(std::sync::atomic::Ordering::Relaxed);

    let now = Instant::now();

    let mut buffer: Vec<u8> = vec![128; (tile_rect.size.x * tile_rect.size.y) as usize];
    let width = img_size.x as f64;
    let height = img_size.y as f64;

    // center
    let offset = Vec2f64::new(fractal_offset.x + 0.74, fractal_offset.y);
    let scale = fractal_scale;

    for y in 0..tile_rect.size.y {
        for x in 0..tile_rect.size.x {
            if x % 32 == 0 {
                if cancel_token.load(std::sync::atomic::Ordering::Relaxed) != cancel_token_value {
                    return Err(anyhow!("Cancelled"));
                }
            }

            let cx = ((x + tile_rect.pos.x) as f64) / width;
            let cy = ((y + tile_rect.pos.y) as f64) / height;

            let cx = (cx - 0.5) / scale - offset.x;
            let cy = (cy - 0.5) / scale - offset.y;

            let c: Complex<f64> = Complex::new(cx, cy);
            let mut z: Complex<f64> = Complex::new(0.0, 0.0);

            let mut it: u32 = 0;
            const MAX_IT: u32 = 256;

            while z.norm() <= 8.0 && it <= MAX_IT {
                z = z * z + c;
                it += 1;
            }

            buffer[(y * tile_rect.size.x + x) as usize] = it as u8;
        }
    }

    if false {
        let elapsed = now.elapsed();
        let target = Duration::from_millis(100);
        if elapsed < target {
            // tokio::time::sleep(target - elapsed).await;
            // thread::sleep(target - elapsed);
        }
    }

    Ok(buffer)
}
