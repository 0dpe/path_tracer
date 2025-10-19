// these structs match with Rust side definitions

struct GpuCamera {
    position: vec4<f32>, // camera position
    lower_left_corner: vec4<f32>, // lower-left pixel coordinate of image plane in world space
    horizontal: vec4<f32>, // vector that spans the full x of image plane in world space
    vertical: vec4<f32>, // vector that spans the full y of image plane in world space
};

struct GpuTriangle {
    p0: vec4<f32>, // p0.w contains material index as a float
    n0: vec4<f32>,
    p1: vec4<f32>,
    n1: vec4<f32>,
    p2: vec4<f32>,
    n2: vec4<f32>,
};

struct GpuMaterial {
    base_color: vec4<f32>,
    emissive: vec4<f32>, // only the first value is used for now as a scalar
};


struct Ray {
    origin: vec3<f32>, // camera origin position in world space
    // normalized, direction going from camera origin to a point on the image plane
    direction: vec3<f32>,
    // matching y = m * x + b, point_along_this_ray = direction * t + origin
};

struct HitRecord {
    t: f32, // distance along the ray where a triangle is hit
    p: vec3<f32>, // world space point where the ray intersects a triangle
    normal: vec3<f32>, // normalized, vector for the normal
    material_index: u32,
    front_face: bool, // whether the triangle is front facing or not
};

@group(0) @binding(0) var screen: texture_storage_2d<rgba16float, write>;
@group(0) @binding(1) var<storage, read> triangles: array<GpuTriangle>;
@group(0) @binding(2) var<storage, read> materials: array<GpuMaterial>;
@group(0) @binding(3) var<uniform> camera: GpuCamera;


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
    return ray;
}

// Möller–Trumbore ray–triangle intersection
// given a Ray and a GpuTriangle, does Ray intersect with this GpuTriangle?
// if no, return (-1.0, 0.0, 0.0)
// if yes, return (distance along ray, first barycentric coordinate, second barycentric coordinate)
// there are only 2 barycentric coordinates because if the coordinates are valid, they must sum to 1
// matching https://w.wiki/y6d
fn intersect_triangle(ray: Ray, tri: GpuTriangle) -> vec3<f32> {
    let edge1 = tri.p1.xyz - tri.p0.xyz; // two edges spanning the triangle
    let edge2 = tri.p2.xyz - tri.p0.xyz;
    let ray_cross_edge2 = cross(ray.direction, edge2); // ray_cross_edge2 is perpendicular to ray.direction and edge2
    let det = dot(edge1, ray_cross_edge2); // det measures how non-parallel the ray is to the plane the triangle is on 

    if (det > -0.00001 && det < 0.00001) {
        return vec3<f32>(-1.0, 0.0, 0.0); // ray is parallel to the triangle plane, so an intersection is impossible
    }

    let inv_det = 1.0 / det;
    let s = ray.origin - tri.p0.xyz;
    let u = inv_det * dot(s, ray_cross_edge2);
    if (u < 0.0 || u > 1.0) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }

    let s_cross_edge1 = cross(s, edge1);
    let v = inv_det * dot(ray.direction, s_cross_edge1);
    if (v < 0.0 || u + v > 1.0) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
    
    let t = inv_det * dot(edge2, s_cross_edge1);

    if (t > 0.00001) {
        return vec3<f32>(t, u, v);
    } else {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
}

fn trace(ray: Ray) -> HitRecord {
    var closest_t = 1.0e+20; // init with a large value; used to determine which triangle is hit first
    var hit_rec: HitRecord;
    hit_rec.t = -1.0; // init t with a negative value, which means that there's no intersection found yet

    let num_triangles = arrayLength(&triangles);
    // for the ray, check intersection with all triangles
    for (var i: u32 = 0u; i < num_triangles; i = i + 1u) {
        let tri = triangles[i];
        let intersection = intersect_triangle(ray, tri);
        let t = intersection.x;

        if (t > 0.0 && t < closest_t) {
            closest_t = t;
            hit_rec.t = t;
            hit_rec.p = ray.direction * t + ray.origin; // this matches f(x) = m * x + b, but in this case in 3D, f(x) outputs a vec3 point, which is the intersection point
            hit_rec.material_index = u32(tri.p0.w); // extract material index from p0.w
            
            // interpolate normal using barycentric coordinates and normals
            let u = intersection.y;
            let v = intersection.z;
            // only 2 barycentric coordinates are given, but we know that the point is valid inside the triangle so all coordinates must sum to 1
            // so, the third barycentric coordinate can just be computed
            let w = 1.0 - u - v;
            let interpolated_normal = normalize(w * tri.n0.xyz + u * tri.n1.xyz + v * tri.n2.xyz);
            
            // determine face orientation
            if (dot(ray.direction, interpolated_normal) < 0.0) {
                hit_rec.front_face = true;
                hit_rec.normal = interpolated_normal;
            } else {
                hit_rec.front_face = false;
                hit_rec.normal = -interpolated_normal;
            }
        }
    }
    return hit_rec;
}

@compute @workgroup_size(8, 8, 1)
fn compute_main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // each global_id comes from compute_pass.dispatch_workgroups() in Rust
    // each global_id.x and global_id.y should yield a pixel on the texture/surface

    let screen_dims = vec2<f32>(textureDimensions(screen));
    
    if (global_id.x >= u32(screen_dims.x) || global_id.y >= u32(screen_dims.y)) {
        return;
    }

    // convert the x and y on the texture/surface to normalized values between 0 and 1
    // + 0.5 to realign each texel to the center of the texel 
    let uv = vec2<f32>(f32(global_id.x) + 0.5, f32(global_id.y) + 0.5) / screen_dims;
    
    let ray = generate_camera_ray(uv);
    let hit = trace(ray);
    
    var final_color = vec3<f32>(0.0, 0.0, 0.0); // color for if no triangles are hit by a ray

    // if there's no intersection between the ray and a triangle, then t = -1.0
    if (hit.t > 0.0) {
        let mat = materials[hit.material_index];
        final_color = mat.base_color.rgb * (1.0 + mat.emissive.r); // only the first value in emissive is used for now
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