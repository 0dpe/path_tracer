// these structs match with Rust side definitions

struct GpuTriangleGeometry {
    p0: vec4<f32>, // p0.w contains material index as a float
    p1: vec4<f32>,
    p2: vec4<f32>,
};

struct BvhNode {
    aabb_min: vec3<f32>,
    left_first: u32,
    aabb_max: vec3<f32>,
    prim_count: u32,
}

struct GpuTriangleAttribute {
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
};

struct GpuMaterial {
    base_color: vec4<f32>,
    emissive: vec4<f32>, // r, g, b, strength
};

struct GpuCamera {
    position: vec4<f32>, // camera position
    lower_left_corner: vec4<f32>, // lower-left pixel coordinate of image plane in world space
    horizontal: vec4<f32>, // vector that spans the full x of image plane in world space
    vertical: vec4<f32>, // vector that spans the full y of image plane in world space
};

@group(0) @binding(0) var screen: texture_storage_2d<rgba16float, write>;
@group(1) @binding(0) var<storage, read> triangles_geo: array<GpuTriangleGeometry>;
@group(1) @binding(1) var<storage, read> bvh_nodes: array<BvhNode>;
@group(1) @binding(2) var<storage, read> triangles_attr: array<GpuTriangleAttribute>;
@group(1) @binding(3) var<storage, read> materials: array<GpuMaterial>;
@group(1) @binding(4) var<uniform> camera: GpuCamera;

struct Ray {
    origin: vec3<f32>, // camera origin position in world space
    // normalized, direction going from camera origin to a point on the image plane
    direction: vec3<f32>,
    // matching y = m * x + b, point_along_this_ray = direction * t + origin
    inv_dir: vec3<f32>, // 1.0/direction; precomputed for fast AABB intersections
};

struct HitRecord {
    t: f32, // distance along the ray where a triangle is hit
    p: vec3<f32>, // world space point where the ray intersects a triangle
    normal: vec3<f32>, // normalized, vector for the normal
    material_index: u32,
    front_face: bool, // whether the triangle is front facing or not
};

// creates a Ray with origin at camera and direction to a given image plane point
// the image plane point is defined by uv, which has x and y normalized to 0 to 1
// for example, uv = (0, 1) represents the upper left corner of the image plane
fn generate_camera_ray(uv: vec2<f32>) -> Ray {
    var ray: Ray;
    ray.origin = camera.position.xyz;
    // normalized vector going from camera origin to the image plane coordinate in world space
    ray.direction = normalize(
        camera.lower_left_corner.xyz + uv.x * camera.horizontal.xyz + uv.y * camera.vertical.xyz - ray.origin
    );

    // https://www.w3.org/TR/WGSL/#differences-from-ieee754 division by zero naturally results in infinity, which works with AABB math
    ray.inv_dir = 1.0 / ray.direction;
    return ray;
}

// ray-AABB intersection (slab method)
fn hit_aabb(ray: Ray, aabb_min: vec3<f32>, aabb_max: vec3<f32>, t_max: f32) -> bool {
    let t0 = (aabb_min - ray.origin) * ray.inv_dir;
    let t1 = (aabb_max - ray.origin) * ray.inv_dir;

    let tmin = min(t0, t1);
    let tmax = max(t0, t1);

    let t_near = max(max(tmin.x, tmin.y), tmin.z);
    let t_far = min(min(tmax.x, tmax.y), tmax.z);

    return t_near <= t_far && t_far > 0.0 && t_near < t_max;
}

fn trace(ray: Ray) -> HitRecord {
    var closest_t = 1.0e+20;
    var hit_idx: i32 = -1;
    var hit_u: f32 = 0.0;
    var hit_v: f32 = 0.0;

    // fixed-size stack for BVH traversal (depth 64 is sufficient for millions of primitives)
    var stack: array<u32, 64>;
    var stack_ptr: u32 = 0u;

    // push root node
    stack[stack_ptr] = 0u;
    stack_ptr += 1u;

    while stack_ptr > 0u {
        // pop node
        stack_ptr -= 1u;
        let node_idx = stack[stack_ptr];
        let node = bvh_nodes[node_idx];

        // if ray misses the bounding box or is further than the closest hit, skip it
        if !hit_aabb(ray, node.aabb_min, node.aabb_max, closest_t) {
            continue;
        }

        if node.prim_count > 0u { // leaf node; intersect with primitives
            for (var i = 0u; i < node.prim_count; i += 1u) {
                let tri_idx = node.left_first + i;

                // Möller–Trumbore algorithm: implementation based on https://w.wiki/y6d
                let p0 = triangles_geo[tri_idx].p0.xyz;
                let edge1 = triangles_geo[tri_idx].p1.xyz - p0; // two edges spanning the triangle
                let edge2 = triangles_geo[tri_idx].p2.xyz - p0;

                let ray_cross_edge2 = cross(ray.direction, edge2); // ray_cross_edge2 is perpendicular to ray.direction and edge2
                let det = dot(edge1, ray_cross_edge2); // det measures how non-parallel the ray is to the plane the triangle is on 

                if abs(det) < 0.000001 {
                    continue; // ray is parallel to the triangle plane, so an intersection is impossible
                }

                let inv_det = 1.0 / det;
                let s = ray.origin - p0;
                let u = dot(s, ray_cross_edge2) * inv_det;
                if u < 0.0 || u > 1.0 { continue; }

                let s_cross_edge1 = cross(s, edge1);
                let v = dot(ray.direction, s_cross_edge1) * inv_det;
                if v < 0.0 || u + v > 1.0 { continue; }

                let t = dot(edge2, s_cross_edge1) * inv_det;
                if t > 0.0001 && t < closest_t {
                    closest_t = t;
                    hit_idx = i32(tri_idx);
                    hit_u = u;
                    hit_v = v;
                }
            }
        } else { // internal node; push children onto stack
            // left_first contains the index of the left child; right child is contiguous at left_first + 1
            stack[stack_ptr] = node.left_first + 1u; // push right
            stack_ptr += 1u;
            stack[stack_ptr] = node.left_first;      // push left
            stack_ptr += 1u;
        }
    }

    var hit_rec: HitRecord;
    hit_rec.t = -1.0;

    if hit_idx != -1 {
        let geo = triangles_geo[u32(hit_idx)];
        let attr = triangles_attr[u32(hit_idx)];

        hit_rec.t = closest_t;
        hit_rec.p = ray.direction * closest_t + ray.origin; // this matches f(x) = m * x + b, but in this case in 3D, f(x) outputs a vec3 point, which is the intersection point
        hit_rec.material_index = u32(geo.p0.w); // extract material index from p0.w

        // interpolate normal using barycentric coordinates and normals
        // only 2 barycentric coordinates are given, but we know that the point is valid inside the triangle so all coordinates must sum to 1
        // so, the third barycentric coordinate can just be computed
        let w = 1.0 - hit_u - hit_v;
        let interpolated_normal = normalize(w * attr.n0.xyz + hit_u * attr.n1.xyz + hit_v * attr.n2.xyz);

        // determine face orientation
        if dot(ray.direction, interpolated_normal) < 0.0 {
            hit_rec.front_face = true;
            hit_rec.normal = interpolated_normal;
        } else {
            hit_rec.front_face = false;
            hit_rec.normal = -interpolated_normal;
        }
    }

    return hit_rec;
}

@compute @workgroup_size(8, 8, 1)
fn compute_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // each global_id comes from compute_pass.dispatch_workgroups() in Rust
    // each global_id.x and global_id.y should yield a pixel on the texture/surface

    let screen_dims = vec2<f32>(textureDimensions(screen));

    if global_id.x >= u32(screen_dims.x) || global_id.y >= u32(screen_dims.y) {
        return;
    }

    // convert the x and y on the texture/surface to normalized values between 0 and 1
    // + 0.5 to realign each texel to the center of the texel 
    let uv = vec2<f32>(f32(global_id.x) + 0.5, f32(global_id.y) + 0.5) / screen_dims;

    let ray = generate_camera_ray(uv);
    let hit = trace(ray);

    var final_color = vec3<f32>(0.0, 0.0, 0.0); // color for if no triangles are hit by a ray

    // if there's no intersection between the ray and a triangle, then t = -1.0
    if hit.t > 0.0 {
        let mat = materials[hit.material_index];
        final_color = mat.base_color.rgb + mat.emissive.rgb * mat.emissive.w;
    }

    textureStore(screen, global_id.xy, vec4<f32>(final_color, 1.0));
}

@vertex
fn vs_main(@builtin(vertex_index) vert_index: u32) -> @builtin(position) vec4<f32> {
    let pos = array(
        vec2<f32>(-1.0, -1.0), // clip space range [-1, 1] so extending to 3 stretches the triangle to cover the clip space
        vec2<f32>(3.0, -1.0), // https://webgpufundamentals.org/webgpu/lessons/webgpu-large-triangle-to-cover-clip-space.html
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(pos[vert_index], 0.0, 1.0);
}

@group(0) @binding(0) var output_texture: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

@fragment
fn fs_main(@builtin(position) frag_position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag_position.xy / vec2<f32>(textureDimensions(output_texture));
    let color = textureSample(output_texture, tex_sampler, uv);

    // sRGB gamma conversion
    let srgb_color = select(
        color.rgb * 12.92,
        pow(color.rgb, vec3<f32>(1.0 / 2.4)) * 1.055 - 0.055,
        color.rgb > vec3<f32>(0.0031308)
    );
    return vec4<f32>(srgb_color, color.a);
}