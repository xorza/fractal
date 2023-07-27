use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use num_complex::Complex;
use rayon::iter::IndexedParallelIterator;
use rayon::iter::IntoParallelRefMutIterator;
use rayon::iter::ParallelIterator;

use crate::math::{Vec2f64, Vec2u32};

pub fn mandelbrot(
    size: Vec2u32,
    offset: Vec2f64,
    scale: f64,
    cancel_token: Arc<AtomicU32>,
) -> anyhow::Result<Vec<u8>>
{
    // let mut buffer: Vec<u8> = vec![0; (size.x * size.y) as usize];
    let width = size.x as f64;
    let height = size.y as f64;
    let aspect = width / height;

    let cancel_token_value = cancel_token.load(std::sync::atomic::Ordering::Relaxed);

    let start = std::time::Instant::now();

    // center
    let offset = Vec2f64::new(offset.x + 0.4, offset.y) * 2.3;
    let scale = scale * 2.3;

    let mut buffers = (0..size.y)
        .map(|_| Vec::with_capacity(size.x as usize))
        .collect::<Vec<Vec<u8>>>();
    buffers
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(y, row)| {
            if cancel_token.load(std::sync::atomic::Ordering::Relaxed) != cancel_token_value {
                return Err(());
            }

            for x in 0..size.x {
                let x = x as f64 / width;
                let y = y as f64 / height;

                let cx = (x - 0.5) * scale - offset.x;
                let cy = (y - 0.5) * scale - offset.y;

                let cx = cx * aspect;

                let c: Complex<f64> = Complex::new(cx, cy);
                let mut z: Complex<f64> = Complex::new(0.0, 0.0);

                let mut it: u32 = 0;
                const MAX_IT: u32 = 256;

                while z.norm() <= 8.0 && it <= MAX_IT {
                    z = z * z + c;
                    it += 1;
                }

                row.push(it as u8);
            }

            Ok(())
        })
        .map_err(|_| anyhow::anyhow!("Cancelled"))?;

    let buffer: Vec<u8> = buffers
        .into_iter()
        .flatten()
        .collect::<Vec<u8>>();

    let elapsed = start.elapsed();
    println!("Mandelbrot rendered in {}ms", elapsed.as_millis());

    // if elapsed.as_millis() < 500 {
    //     let ms = 500 - elapsed.as_millis() as u64;
    //     thread::sleep(std::time::Duration::from_millis(ms));
    // }

    Ok(buffer)
}
