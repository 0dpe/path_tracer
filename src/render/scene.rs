// this module is only used by state.rs
// load a .glb glTF 2.0 file and format the data
// on both native and wasm, loading is at runtime
// this allows the glTF file to change without having to rebuild the WASM

#[derive(Debug)]
pub struct Scene {
    pub geometries: Vec<GpuTriangleGeometry>,
    pub attributes: Vec<GpuTriangleAttribute>,
    pub materials: Vec<GpuMaterial>,
    pub bvh_nodes: Vec<GpuBvhNode>,
    camera: Camera,
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

// GpuTriangleGeometry and GpuTriangleAttribute are separate structs the shader fetches vertices much more frequently than normals
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)] // Clone and Copy are required by Pod
pub struct GpuTriangleGeometry {
    // bytemuck::Pod requires alignment without padding
    // shader also requires padding https://www.w3.org/TR/WGSL/#alignment-and-size
    // although glam::Vec3A is 16 byte aligned, it has padding
    // except for p0, for every other point and normal only 3 values are useful; the last value is 0.0
    // for p0 only, the last value is meaningful, which points to the index of a GpuMaterial
    // meshes are not passed to the GPU; instead, individual triangles themselves are passed
    // each triangle thus has a material index, which indicates which mesh the triangle is from
    p0: glam::Vec4,
    p1: glam::Vec4,
    p2: glam::Vec4,
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuTriangleAttribute {
    n0: glam::Vec4,
    n1: glam::Vec4,
    n2: glam::Vec4,
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuMaterial {
    base_color: glam::Vec4,
    emissive: glam::Vec4, // emissive color in rgb, then strength in the 4th value
}

// if prim_count == 0, the node is an internal node, and left_first is the index of the left child
// the right child is guaranteed to be immediately after left_first at left_first + 1
// if prim_count > 0, the node is a leaf, and left_first is the starting offset into the geometry buffers
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuBvhNode {
    aabb_min: [f32; 3],
    left_first: u32,
    aabb_max: [f32; 3],
    prim_count: u32,
}

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuCamera {
    position: glam::Vec4,          // camera position, same as in Camera
    lower_left_corner: glam::Vec4, // lower-left pixel coordinate of image plane in world space
    horizontal: glam::Vec4,        // vector that spans the full x of image plane in world space
    vertical: glam::Vec4,          // vector that spans the full y of image plane in world space
}

// internal BVH helper structs
#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: glam::Vec3,
    max: glam::Vec3,
}
impl Aabb {
    const fn new() -> Self {
        Self {
            min: glam::Vec3::splat(f32::INFINITY),
            max: glam::Vec3::splat(f32::NEG_INFINITY),
        }
    }
    fn grow(&mut self, p: glam::Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    #[allow(clippy::use_self)]
    fn union(&mut self, other: &Aabb) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }
    fn area(&self) -> f32 {
        let e = (self.max - self.min).max(glam::Vec3::ZERO);
        e.z.mul_add(e.x, e.x.mul_add(e.y, e.y * e.z)) // e.x * e.y + e.y * e.z + e.z * e.x, but using mul_add for better precision and performance
    }
}
#[derive(Clone, Copy)]
struct PrimitiveInfo {
    aabb: Aabb,
    centroid: glam::Vec3,
}

// private helper function used in new()
// not included directly inside new because conditional compilation with variable scopes would get messy
#[allow(clippy::unused_async)]
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

        let mut geometries = Vec::new();
        let mut attributes = Vec::new();
        let mut materials = Vec::new();
        let mut prim_infos = Vec::new();

        // the 2 extensions enabled are KHR_materials_emissive_strength and KHR_materials_specular
        // in Blender, with principled BSDF, if emission strength is larger than 1.0, KHR_materials_emissive_strength will automatically be used in the exported glTF file
        document.extensions_used().for_each(|s| {
            log::info!("glTF used extension: {s}");
        });

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

                        let indices: Vec<u32> = reader.read_indices().map_or_else(
                            || (0u32..positions.len() as u32).collect(),
                            |read_indices| read_indices.into_u32().collect(),
                        );

                        let material_index = materials.len() as f32;
                        materials.push(GpuMaterial {
                            base_color: glam::Vec4::from(
                                primitive
                                    .material() // for more matieral properties, this is where to get them
                                    .pbr_metallic_roughness() // for metallic and roughness properties, and the texture, this is where to get them
                                    .base_color_factor(),
                            ),
                            emissive: glam::Vec4::new(
                                primitive.material().emissive_factor()[0],
                                primitive.material().emissive_factor()[1],
                                primitive.material().emissive_factor()[2],
                                primitive.material().emissive_strength().unwrap_or_default(),
                            ),
                        });

                        for chunk in indices.chunks_exact(3) {
                            let i0 = chunk[0] as usize;
                            let i1 = chunk[1] as usize;
                            let i2 = chunk[2] as usize;

                            let p0 = model_mat.transform_point3(positions[i0]);
                            let p1 = model_mat.transform_point3(positions[i1]);
                            let p2 = model_mat.transform_point3(positions[i2]);

                            let n0 = normal_mat.transform_vector3(normals[i0]).normalize();
                            let n1 = normal_mat.transform_vector3(normals[i1]).normalize();
                            let n2 = normal_mat.transform_vector3(normals[i2]).normalize();

                            // calculate AABB and centroid for the BVH builder
                            prim_infos.push(PrimitiveInfo {
                                aabb: Aabb {
                                    min: p0.min(p1).min(p2),
                                    max: p0.max(p1).max(p2),
                                },
                                centroid: (p0 + p1 + p2) / 3.0,
                            });

                            geometries.push(GpuTriangleGeometry {
                                p0: glam::Vec4::from((p0, material_index)),
                                p1: glam::Vec4::from((p1, 0.0)),
                                p2: glam::Vec4::from((p2, 0.0)),
                            });

                            attributes.push(GpuTriangleAttribute {
                                n0: glam::Vec4::from((n0, 0.0)),
                                n1: glam::Vec4::from((n1, 0.0)),
                                n2: glam::Vec4::from((n2, 0.0)),
                            });
                        }
                    }
                }
            }
        }

        let mut bvh_nodes = vec![GpuBvhNode {
            aabb_min: [0.0; 3],
            left_first: 0,
            aabb_max: [0.0; 3],
            prim_count: 0,
        }];

        log::info!(
            "Loaded scene with {} triangles, {} materials; starting BVH construction",
            geometries.len(),
            materials.len()
        );

        let bvh_construction_start = web_time::Instant::now();

        if !prim_infos.is_empty() {
            let prim_count = prim_infos.len();
            Self::update_node_bounds(0, &mut bvh_nodes, &prim_infos, 0, prim_count);
            Self::subdivide(
                0,
                &mut bvh_nodes,
                &mut prim_infos,
                &mut geometries,
                &mut attributes,
                0,
                prim_count,
            );
        }

        log::info!(
            "Finished BVH construction with {} nodes, took {:?}",
            bvh_nodes.len(),
            bvh_construction_start.elapsed()
        );

        Ok(Self {
            geometries,
            attributes,
            materials,
            bvh_nodes,
            // with this camera orientation, Blender exported .glb files will appear in this orientation expressed in Blender's coordinate system:
            // +Z upward, +X rightward, +Y into the screen
            // this happens when the checkbox "+Y Up" is checked when exporting a .glb file in Blender
            camera: Camera {
                position: glam::Vec3::new(0.0, 1.0, 2.5), // TODO auto calculate a better starting position based on the scene's bounding box
                fov_y: 90.0,
                aspect_ratio: 1.0,
                yaw: 0.0, // facing -Z, which is into the screen
                pitch: 0.0,
            },
        })
    }

    // updates bounding box for a given node based on triangles it spans
    fn update_node_bounds(
        node_idx: usize,
        nodes: &mut [GpuBvhNode],
        prim_infos: &[PrimitiveInfo],
        start: usize,
        end: usize,
    ) {
        let mut aabb = Aabb::new();
        for info in &prim_infos[start..end] {
            aabb.union(&info.aabb);
        }
        nodes[node_idx].aabb_min = aabb.min.to_array();
        nodes[node_idx].aabb_max = aabb.max.to_array();
    }
    // binning SAH sub-divider
    fn subdivide(
        node_idx: usize,
        nodes: &mut Vec<GpuBvhNode>,
        prim_infos: &mut [PrimitiveInfo],
        geometries: &mut [GpuTriangleGeometry],
        attributes: &mut [GpuTriangleAttribute],
        start: usize,
        end: usize,
    ) {
        let prim_count = end - start;
        // if there are 2 or fewer triangles, make this node a leaf; otherwise, keep subdividing
        if prim_count <= 2 {
            nodes[node_idx].left_first = start as u32;
            nodes[node_idx].prim_count = prim_count as u32;
            return;
        }

        // split based on not the edges of triangles; calculate bounding box that encapsulates only the centroids
        let mut centroid_bounds = Aabb::new();
        for info in &prim_infos[start..end] {
            centroid_bounds.grow(info.centroid);
        }

        const BINS: usize = 8;
        let mut best_axis = 0;
        let mut best_split = 0;
        let mut best_cost = f32::MAX;

        for axis in 0..3 {
            // for axis in x, y, z
            let bounds_min = centroid_bounds.min[axis];
            let bounds_max = centroid_bounds.max[axis];
            #[allow(clippy::float_cmp)] // there should be no precision drift here
            if bounds_min == bounds_max {
                continue;
            } // all primitive centroids are overlapping on this axis

            let scale = BINS as f32 / (bounds_max - bounds_min);

            #[derive(Clone, Copy)]
            struct Bin {
                count: u32,
                bounds: Aabb,
            }
            let mut bins = [Bin {
                count: 0,
                bounds: Aabb::new(),
            }; BINS];

            for info in &prim_infos[start..end] {
                let centroid = info.centroid[axis];
                #[allow(clippy::cast_sign_loss)]
                let mut bin_idx = ((centroid - bounds_min) * scale) as usize;
                bin_idx = bin_idx.min(BINS - 1);
                bins[bin_idx].count += 1;
                bins[bin_idx].bounds.union(&info.aabb);
            }

            let mut left_area = [0.0; BINS - 1];
            let mut left_count = [0; BINS - 1];
            let mut right_area = [0.0; BINS - 1];
            let mut right_count = [0; BINS - 1];

            let mut left_box = Aabb::new();
            let mut left_sum = 0;
            for i in 0..BINS - 1 {
                left_sum += bins[i].count;
                left_box.union(&bins[i].bounds);
                left_count[i] = left_sum;
                left_area[i] = left_box.area();
            }

            let mut right_box = Aabb::new();
            let mut right_sum = 0;
            for i in (1..BINS).rev() {
                right_sum += bins[i].count;
                right_box.union(&bins[i].bounds);
                right_count[i - 1] = right_sum;
                right_area[i - 1] = right_box.area();
            }

            for i in 0..BINS - 1 {
                let cost = (left_count[i] as f32)
                    .mul_add(left_area[i], right_count[i] as f32 * right_area[i]);
                if cost < best_cost {
                    best_cost = cost;
                    best_axis = axis;
                    best_split = i;
                }
            }
        }

        let node_area = {
            let e = glam::Vec3::from_array(nodes[node_idx].aabb_max)
                - glam::Vec3::from_array(nodes[node_idx].aabb_min);
            e.z.mul_add(e.x, e.x.mul_add(e.y, e.y * e.z)) // omitting 2.0 coefficient
        };
        let leaf_cost = prim_count as f32 * node_area;

        // if making it a leaf is cheaper than the best SAH split, terminate here
        if best_cost >= leaf_cost {
            nodes[node_idx].left_first = start as u32;
            nodes[node_idx].prim_count = prim_count as u32;
            return;
        }

        // partitioning primitives, geometries, and attributes arrays in place
        let bounds_min = centroid_bounds.min[best_axis];
        let bounds_max = centroid_bounds.max[best_axis];
        let scale = BINS as f32 / (bounds_max - bounds_min);

        let mut left = start;
        let mut right = end - 1;

        while left <= right {
            let centroid = prim_infos[left].centroid[best_axis];
            #[allow(clippy::cast_sign_loss)]
            let mut bin_idx = ((centroid - bounds_min) * scale) as usize;
            bin_idx = bin_idx.min(BINS - 1);

            if bin_idx <= best_split {
                left += 1;
            } else {
                prim_infos.swap(left, right);
                geometries.swap(left, right);
                attributes.swap(left, right);
                if right == 0 {
                    break;
                } // safe guard against underflow 
                right -= 1;
            }
        }

        let split_idx = left;

        // edge case: floats caused a weird partition leaving one side completely empty
        if split_idx == start || split_idx == end {
            nodes[node_idx].left_first = start as u32;
            nodes[node_idx].prim_count = prim_count as u32;
            return;
        }

        // create child nodes contiguously (left, right)
        let left_child_idx = nodes.len();
        nodes.push(GpuBvhNode {
            aabb_min: [0.0; 3],
            left_first: 0,
            aabb_max: [0.0; 3],
            prim_count: 0,
        });
        let right_child_idx = nodes.len();
        nodes.push(GpuBvhNode {
            aabb_min: [0.0; 3],
            left_first: 0,
            aabb_max: [0.0; 3],
            prim_count: 0,
        });

        nodes[node_idx].left_first = left_child_idx as u32;
        nodes[node_idx].prim_count = 0; // 0 signals non-leaf internal node

        Self::update_node_bounds(left_child_idx, nodes, prim_infos, start, split_idx);
        Self::update_node_bounds(right_child_idx, nodes, prim_infos, split_idx, end);

        // TODO: investigate if rayon will make this faster or slower; parallelism overhead might outweigh the speedup for small to medium scenes
        Self::subdivide(
            left_child_idx,
            nodes,
            prim_infos,
            geometries,
            attributes,
            start,
            split_idx,
        );
        Self::subdivide(
            right_child_idx,
            nodes,
            prim_infos,
            geometries,
            attributes,
            split_idx,
            end,
        );
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
