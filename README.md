# Mandelbrot explorer

## Description
Desktop UI application for exploring the Mandelbrot set. Draggable and zoomable.
Calculation is done on CPU, no Simd yet.
Multithreaded.
Draft drag and zoom done on GPU.

Written on Rust. Uses winit, wgpu, rayon and tokio.
Runs pretty smooth on my Macbook Air M2 2022.

![doc/Screen Recording 2023-08-18 at 5.35.27 PM.gif](doc/Screen Recording 2023-08-18 at 5.35.27 PM.gif)