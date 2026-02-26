// this module is only used by state.rs
// load a .glb glTF 2.0 file and format the data
// on both native and wasm, loading is at runtime
// this allows the glTF file to change without having to rebuild the WASM

#[derive(Debug)]
pub struct Scene {
    meshes: Vec<Mesh>,
    camera: Camera,
}

#[derive(Debug)]
// each Vertex struct represents just a point
struct Vertex {
    position: glam::Vec3, // x, y, z
    normal: glam::Vec3,   // x, y, z (normalized)
}

#[derive(Debug)]
struct Mesh {
    vertices: Vec<Vertex>,
    // indices is the sequence of vertices
    // if there are 6 Vertex structs in vertices, there would be 2 triangles, which means 6 values in indices
    // for triangle A and B, the indices would be parsed like so: A1, A2, A3, B1, B2, B3
    indices: Vec<u32>,
    base_color: glam::Vec4, // albedo color, should be RGBA
    emissive: f32,          // emissivity intensity
}

#[derive(Debug)]
struct Camera {
    position: glam::Vec3, // camera position x, y, z in world space
    // -Z into the screen, +Y up the screen, +X to the right of the screen
    // focus distance is not needed since this is a pinhole camera; focus distance is implicitly 1
    fov_y: f32,        // vertical fov in degrees
    aspect_ratio: f32, // width/height of image plane
    yaw: f32,
    pitch: f32,
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)] // Clone and Copy are required by Pod
pub struct GpuTriangle {
    // bytemuck::Pod requires alignment without padding
    // shader also requires padding https://www.w3.org/TR/WGSL/#alignment-and-size
    // although glam::Vec3A is 16 byte aligned, it has padding
    // except for p0, for every other point and normal only 3 values are useful; the last value is 0.0
    // for p0 only, the last value is meaningful, which points to the index of a GpuMaterial
    // meshes are not passed to the GPU; instead, individual triangles themselves are passed
    // each triangle thus has a material index, which indicates which mesh the triangle is from
    p0: glam::Vec4,
    n0: glam::Vec4,
    p1: glam::Vec4,
    n1: glam::Vec4,
    p2: glam::Vec4,
    n2: glam::Vec4,
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuMaterial {
    base_color: glam::Vec4,
    // although emissive is just a number, use a Vec4 to satisfy alignment
    // only the first value is meaningful
    emissive: glam::Vec4,
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuCamera {
    position: glam::Vec4,          // camera position, same as in Camera
    lower_left_corner: glam::Vec4, // lower-left pixel coordinate of image plane in world space
    horizontal: glam::Vec4,        // vector that spans the full x of image plane in world space
    vertical: glam::Vec4,          // vector that spans the full y of image plane in world space
}

// private helper function used in new()
// not included directly inside new because conditional compilation with variable scopes would get messy
async fn load_gltf_bytes(path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(std::fs::read(path)?)
    }

    // on web, the glTF file is fetched over HTTP
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast; // trait for dyn_into()

        let response: web_sys::Response = wasm_bindgen_futures::JsFuture::from(
            web_sys::window()
                .ok_or("no window object")? // ok_or() converts Option<> to Result<> with error message
                .fetch_with_str(path),
        )
        .await
        .map_err(|e| format!("{:?}", e))?
        .dyn_into()
        .map_err(|e| format!("{:?}", e))?;

        if !response.ok() {
            return Err(format!("network error: status {}", response.status()).into());
        }

        let u8_array = js_sys::Uint8Array::new(
            &wasm_bindgen_futures::JsFuture::from(
                response.array_buffer().map_err(|e| format!("{:?}", e))?,
            )
            .await
            .map_err(|e| format!("{:?}", e))?,
        );
        let mut bytes = vec![0u8; u8_array.length() as usize];
        u8_array.copy_to(&mut bytes[..]);
        Ok(bytes)
    }
}

impl Scene {
    pub async fn new(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let (document, buffers, _images) = gltf::import_slice(load_gltf_bytes(path).await?)?;

        let mut meshes: Vec<Mesh> = Vec::new();

        // collect meshes referenced by nodes
        for node in document.default_scene().map_or_else(
            || document.nodes().collect::<Vec<_>>(),
            |scene| scene.nodes().collect::<Vec<_>>(),
        ) {
            // compute this node's transform matrix
            let model_mat = match node.transform() {
                gltf::scene::Transform::Matrix { matrix } => {
                    glam::Mat4::from_cols_array_2d(&matrix)
                }
                gltf::scene::Transform::Decomposed {
                    translation,
                    rotation,
                    scale,
                } => {
                    glam::Mat4::from_translation(glam::Vec3::from(translation))
                        * glam::Mat4::from_quat(glam::Quat::from_array(rotation))
                        * glam::Mat4::from_scale(glam::Vec3::from(scale))
                }
            };

            let normal_mat = model_mat.inverse().transpose();

            // get the mesh data if this node references a mesh
            if let Some(mesh) = node.mesh() {
                // primitives are the only useful data in a gltf::Mesh for this renderer
                // usually there's only one primitive for a mesh
                for primitive in mesh.primitives() {
                    // only process the primitive if it's triangles
                    if primitive.mode() == gltf::mesh::Mode::Triangles {
                        // a reader can read the data of a mesh primitive
                        let reader =
                            primitive.reader(|buffer| Some(&buffers[buffer.index()].0[..]));

                        let positions: Vec<glam::Vec3> = match reader.read_positions() {
                            // type annotation necessary
                            Some(it) => it.map(glam::Vec3::from_array).collect(), // collect the iterator
                            None => continue, // if there are no positions (vertex positions), skip this primitive
                        };

                        let normals: Vec<glam::Vec3> = match reader.read_normals() {
                            Some(it) => it.map(glam::Vec3::from_array).collect(),
                            None => continue, // if there are no normals, skip this primitive
                        };

                        // build vertices, which holds Vertex structs
                        let mut vertices: Vec<Vertex> = Vec::with_capacity(positions.len());
                        for i in 0..positions.len() {
                            vertices.push(Vertex {
                                position: model_mat.transform_point3(positions[i]),
                                normal: normal_mat.transform_vector3(normals[i]).normalize(),
                            });
                        }

                        meshes.push(Mesh {
                            vertices,
                            indices: reader.read_indices().map_or_else(
                                || (0u32..positions.len() as u32).collect(),
                                |read_indices| read_indices.into_u32().collect(),
                            ),
                            base_color: glam::Vec4::from(
                                primitive
                                    .material() // for more matieral properties, this is where to get them
                                    .pbr_metallic_roughness() // for metallic and roughness properties, and the texture, this is where to get them
                                    .base_color_factor(),
                            ),
                            emissive: 1.0, // TODO actually parse emissivity later
                        });
                    }
                }
            }
        }

        Ok(Self {
            meshes,
            // with this camera orientation, Blender exported .glb files will appear in this orientation expressed in Blender's coordinate system:
            // +Z upward, +X rightward, +Y into the screen
            // this happens when the checkbox "+Y Up" is checked when exporting a .glb file in Blender
            camera: Camera {
                position: glam::Vec3::new(-1.0, 0.0, 7.0), // TODO calculate a better starting position
                fov_y: 90.0,
                aspect_ratio: 1.0,
                yaw: 0.0,
                pitch: 0.0,
            },
        })
    }

    pub fn resize_camera_aspect_ratio(&mut self, width: f32, height: f32) {
        self.camera.aspect_ratio = width / height;
    }

    pub fn move_camera(
        &mut self,
        pressed_keys: &std::collections::HashSet<winit::keyboard::KeyCode>,
        horizontal_speed: f32,
        vertical_speed: f32,
    ) -> bool {
        let w = pressed_keys.contains(&winit::keyboard::KeyCode::KeyW);
        let s = pressed_keys.contains(&winit::keyboard::KeyCode::KeyS);
        let a = pressed_keys.contains(&winit::keyboard::KeyCode::KeyA);
        let d = pressed_keys.contains(&winit::keyboard::KeyCode::KeyD);
        let space = pressed_keys.contains(&winit::keyboard::KeyCode::Space);
        let shift = pressed_keys.contains(&winit::keyboard::KeyCode::ShiftLeft);

        if !(w || s || a || d || space || shift) {
            return false;
        }

        let (sin_yaw, cos_yaw) = self.camera.yaw.sin_cos();

        let forward_xz = glam::Vec3::new(-sin_yaw, 0.0, -cos_yaw);
        let right_xz = glam::Vec3::new(cos_yaw, 0.0, -sin_yaw);

        let forward_coeff = (if w { 1.0 } else { 0.0 }) - (if s { 1.0 } else { 0.0 });
        let right_coeff = (if d { 1.0 } else { 0.0 }) - (if a { 1.0 } else { 0.0 });

        let intent = forward_xz * forward_coeff + right_xz * right_coeff;

        let move_xz = if intent.length_squared() > f32::EPSILON {
            intent.normalize() * horizontal_speed
        } else {
            glam::Vec3::ZERO
        };

        let vert_dir = (if space { 1.0 } else { 0.0 }) - (if shift { 1.0 } else { 0.0 });
        self.camera.position += move_xz + glam::Vec3::new(0.0, vert_dir * vertical_speed, 0.0);

        true
    }

    pub fn rotate_camera(
        &mut self,
        dx: f32,
        dy: f32,
        horizontal_sensitivity: f32,
        vertical_sensitivity: f32,
    ) {
        self.camera.yaw -= dx * horizontal_sensitivity;
        self.camera.pitch -= dy * vertical_sensitivity;

        self.camera.pitch = self
            .camera
            .pitch
            .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
    }

    pub fn prepare_gpu_triangle_material(&self) -> (Vec<GpuTriangle>, Vec<GpuMaterial>) {
        let estimated_tri_count = self.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let mut gpu_triangles = Vec::with_capacity(estimated_tri_count);
        let mut gpu_materials = Vec::with_capacity(self.meshes.len());

        for (material_index, mesh) in self.meshes.iter().enumerate() {
            for tri_indices in mesh.indices.chunks_exact(3) {
                let v0 = &mesh.vertices[tri_indices[0] as usize];
                let v1 = &mesh.vertices[tri_indices[1] as usize];
                let v2 = &mesh.vertices[tri_indices[2] as usize];

                gpu_triangles.push(GpuTriangle {
                    p0: glam::Vec4::from((v0.position, material_index as f32)),
                    n0: glam::Vec4::from((v0.normal, 0.0)),
                    p1: glam::Vec4::from((v1.position, 0.0)),
                    n1: glam::Vec4::from((v1.normal, 0.0)),
                    p2: glam::Vec4::from((v2.position, 0.0)),
                    n2: glam::Vec4::from((v2.normal, 0.0)),
                });
            }

            gpu_materials.push(GpuMaterial {
                base_color: mesh.base_color,
                emissive: glam::Vec4::new(mesh.emissive, 0.0, 0.0, 0.0),
            });
        }

        (gpu_triangles, gpu_materials)
    }

    pub fn prepare_gpu_camera(&self) -> GpuCamera {
        let cam = &self.camera;

        let rotation_mat =
            glam::Mat3::from_rotation_y(cam.yaw) * glam::Mat3::from_rotation_x(cam.pitch);

        let image_plane_height = 2.0 * (cam.fov_y.to_radians() / 2.0).tan();

        let horizontal3 = rotation_mat.mul_vec3(glam::Vec3::X).normalize()
            * cam.aspect_ratio
            * image_plane_height;

        let vertical3 = rotation_mat.mul_vec3(glam::Vec3::NEG_Y).normalize() * image_plane_height;

        let forward3 = rotation_mat.mul_vec3(glam::Vec3::NEG_Z).normalize();

        let lower_left_corner3 = cam.position + forward3 - horizontal3 / 2.0 - vertical3 / 2.0;

        GpuCamera {
            position: glam::Vec4::from((cam.position, 0.0)),
            lower_left_corner: glam::Vec4::from((lower_left_corner3, 0.0)),
            horizontal: glam::Vec4::from((horizontal3, 0.0)),
            vertical: glam::Vec4::from((vertical3, 0.0)),
        }
    }
}
