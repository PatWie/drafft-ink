// Copies the Vello texture to the target 1:1, one texel per pixel. egui draws on top in the same pass.
@group(0) @binding(0) var scene: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One oversized triangle that covers the viewport.
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(scene));
    let xy = clamp(vec2<i32>(p.xy), vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
    return textureLoad(scene, xy, 0);
}
