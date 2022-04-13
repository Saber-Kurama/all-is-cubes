// Copyright 2020-2022 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

// Mirrors `struct WgpuCamera` on the Rust side.
struct WgpuCamera {
    [[location(0)]] projection: mat4x4<f32>;
    [[location(1)]] view_matrix: mat4x4<f32>;
    [[location(2)]] view_position: vec3<f32>;
    [[location(3)]] fog_color: vec3<f32>;
    [[location(4)]] fog_mode_blend: f32;
    [[location(5)]] fog_distance: f32;
    [[location(6)]] exposure: f32;
};

// Mirrors `struct WgpuBlockVertex` on the Rust side.
struct WgpuBlockVertex {
    [[location(0)]] position: vec3<f32>;
    [[location(1)]] cube: vec3<f32>;
    [[location(2)]] normal: vec3<f32>;
    [[location(3)]] color_or_texture: vec4<f32>;
    [[location(4)]] clamp_min: vec3<f32>;
    [[location(5)]] clamp_max: vec3<f32>;
};

struct VertexOutput {
    [[builtin(position)]] clip_position: vec4<f32>;
    [[location(0)]] color_or_texture: vec4<f32>;
    [[location(1)]] clamp_min: vec3<f32>;
    [[location(2)]] clamp_max: vec3<f32>;
};

[[group(0), binding(0)]]
var<uniform> camera: WgpuCamera;

[[group(1), binding(0)]]
var block_texture: texture_3d<f32>;
[[group(1), binding(1)]]
var block_sampler: sampler;

[[stage(vertex)]]
fn vs_main(
    input: WgpuBlockVertex,
) -> VertexOutput {
    var out: VertexOutput;
    out.color_or_texture = input.color_or_texture;
    out.clamp_min = input.clamp_min;
    out.clamp_max = input.clamp_max;

    if (true) {
        out.clip_position = camera.projection * camera.view_matrix * vec4<f32>(input.position, 1.0);
    } else {
        // TODO: Old placeholder projection code preserved to reproduce a bug
        // consistently: <https://github.com/gfx-rs/wgpu/issues/2556>
        out.clip_position = vec4<f32>(input.position * vec3<f32>(0.05, 0.05, 0.01), 1.0);
    }

    return out;
}

// Get the vertex color or texel value to display
fn get_diffuse_color(in: VertexOutput) -> vec4<f32> {
    if (in.color_or_texture[3] < -0.5) {
        // Texture coordinates.
        var texcoord: vec3<f32> =
            clamp(in.color_or_texture.xyz, in.clamp_min, in.clamp_max);
        return textureSampleLevel(block_texture, block_sampler, texcoord, 0.0);

        // TODO: implement DEBUG_TEXTURE_EDGE
    } else {
        // Solid color.
        return in.color_or_texture;
    }
}

[[stage(fragment)]]
fn fs_main_opaque(in: VertexOutput) -> [[location(0)]] vec4<f32> {
    var diffuse_color: vec4<f32> = get_diffuse_color(in);
    
    // Exposure/eye adaptation
    diffuse_color = vec4<f32>(diffuse_color.rgb * camera.exposure, diffuse_color.a);

    return diffuse_color;
}

[[stage(fragment)]]
fn fs_main_transparent(in: VertexOutput) -> [[location(0)]] vec4<f32> {
    var diffuse_color: vec4<f32> = get_diffuse_color(in);

    // Exposure/eye adaptation
    diffuse_color = vec4<f32>(diffuse_color.rgb * camera.exposure, diffuse_color.a);

    var color = diffuse_color; // TODO: lighting

    // Multiply color channels by alpha because our blend function choice is premultiplied alpha.
    return vec4<f32>(color.rgb * color.a, color.a);
}