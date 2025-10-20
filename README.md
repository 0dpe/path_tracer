# [path_tracer](https://0dpe.github.io/path_tracer/)

A cross-platform **demo** GPU ray tracer built with Rust and WebGPU, demonstrating modern GPU programming techniques. Runs natively on Windows, macOS, and Linux, as well as in web browsers from the same codebase.

## Why WebGPU?

GPU programming offers massive parallelism for graphics and compute workloads, but has historically been fragmented across incompatible APIs:

* **OpenGL**: Aging API with inconsistent driver support
* **DirectX**: Windows-only
* **Metal**: Apple platforms only
* **Vulkan**: Verbose, complex to learn
* **CUDA & OpenCL**: Compute-focused, vendor-specific or limited adoption

Unlike CPU code that runs anywhere with minimal changes, GPU developers traditionally had to choose between portability and access to modern features.

WebGPU solves this. It's a modern, cross-platform GPU API that provides a single interface across all major platforms and browsers. WebGPU maps efficiently to DirectX 12, Metal, and Vulkan under the hood without sacrificing performance. The W3C WebGPU spec reached a Candidate Recommendation Draft in 2025.

This project demonstrates WebGPU capabilities using **wgpu** (the Rust implementation) and serves as a practical learning resource for modern GPU programming.

## What It Does

This is a real-time GPU ray tracer that:

* Renders 3D scenes loaded from glTF (.glb) files (tested with Blender exported files)
* Runs the same Rust codebase natively (desktop) and on the web (via WebAssembly)
* Uses compute shaders for ray-triangle intersection (Möller-Trumbore algorithm)
* Handles first-person camera controls with keyboard and mouse input
* Demonstrates core WebGPU (wgpu v27) concepts: bind groups, compute/vertex/fragment pipelines, WGSL compute/vertex/fragment shaders, storage textures, etc.

**Current Status**: This is a basic ray tracer skeleton (~1300 lines of code in total). It traces primary rays only and lacks features like recursive path tracing or acceleration structures (BVH).

## Build & Run

### Prerequisites

1. Install [Rust](https://rustup.rs/)
2. For web builds, install wasm-pack:

   ```bash
   cargo install wasm-pack
   ```

3. For web hosting on localhost, use Python's built-in server or install simple-http-server:

   ```bash
   cargo install simple-http-server
   ```

### Native (Desktop)

```bash
git clone https://github.com/0dpe/path_tracer.git
cd path_tracer
cargo run --release
```

**Controls**: Left click the window, then use <kbd>W</kbd><kbd>A</kbd><kbd>S</kbd><kbd>D</kbd> + <kbd>Left Shift</kbd>/<kbd>Space</kbd> to move, mouse to look around. Press <kbd>Esc</kbd> or click again to release cursor.

### Web (Browser)

```bash
git clone https://github.com/0dpe/path_tracer.git
cd path_tracer
wasm-pack build --target web

# serve the folder
simple-http-server
# Or: python -m http.server 8000
```

Open Chrome at `http://localhost:8000` and click on `index.html`. Same controls as native.

> [!NOTE]
> Requires a browser with WebGPU support (Chrome stable, Safari/Firefox with flags enabled as of 2025).

## General Crate Structure

```text
path_tracer/
├── assets/              # .glb scene files
├── src/
│   ├── render/          # WebGPU setup, shaders, scene loading
│   ├── lib.rs           # Window management and event loop
│   └── main.rs          # Native entry point
├── Cargo.toml           # Dependencies and wgpu version
└── index.html           # Webpage
```
