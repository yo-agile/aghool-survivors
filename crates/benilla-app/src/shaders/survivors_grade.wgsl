// The survivors grade: a light ReShade-style polish on the world, before the HUD is drawn over it.
// The stack that modest WoW presets settle on: AMD FidelityFX CAS sharpening (Lottes), vibrance
// (lifts muted colours more than saturated ones), a gentle S-curve, and a touch of warmth to offset
// the blue that vibrance pulls up. No bloom, no grain. It runs on the world's gamma-byte image,
// before the FFXGlow combine decodes it, as ReShade runs on a game's sRGB backbuffer.
// `params.x` is the master amount: 0 is the identity.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
// x amount, y CAS sharpness 0..1, z vibrance, w S-curve blend.
@group(0) @binding(2) var<uniform> params: vec4<f32>;

const WARM: vec3<f32> = vec3<f32>(1.012, 1.0, 0.985);
const VIGNETTE: f32 = 0.08;

fn tap(p: vec2<i32>) -> vec3<f32> {
    let dims = vec2<i32>(textureDimensions(screen_texture));
    let t = textureLoad(screen_texture, clamp(p, vec2<i32>(0), dims - 1), 0).rgb;
    return clamp(t, vec3<f32>(0.0), vec3<f32>(1.0));
}

// FidelityFX CAS, sharpen-only, on the 3×3 cross: the weight shrinks where local contrast is
// already high, so edges sharpen without ringing and flat areas stay clean.
fn cas(p: vec2<i32>, sharpness: f32) -> vec3<f32> {
    let b = tap(p + vec2<i32>(0, -1));
    let d = tap(p + vec2<i32>(-1, 0));
    let e = tap(p);
    let f = tap(p + vec2<i32>(1, 0));
    let h = tap(p + vec2<i32>(0, 1));
    let mn = min(min(min(d, e), min(f, b)), h);
    let mx = max(max(max(d, e), max(f, b)), h);
    let room = min(mn, vec3<f32>(1.0) - mx) / max(mx, vec3<f32>(1e-5));
    let amp = sqrt(clamp(room, vec3<f32>(0.0), vec3<f32>(1.0)));
    let w = amp * (-1.0 / mix(8.0, 5.0, sharpness));
    let out = ((b + d + f + h) * w + e) / (vec3<f32>(1.0) + 4.0 * w);
    return clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
}

// ReShade's Vibrance.fx: saturation scaled by how unsaturated the pixel already is.
fn vibrance(c: vec3<f32>, amount: f32) -> vec3<f32> {
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sat = max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
    return mix(vec3<f32>(l), c, 1.0 + amount * (1.0 - sat));
}

// A soft S-curve: smoothstep, blended in by `amount`, like Curves.fx's gentlest formula.
fn curve(c: vec3<f32>, amount: f32) -> vec3<f32> {
    return mix(c, c * c * (vec3<f32>(3.0) - 2.0 * c), amount);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    let src = textureLoad(screen_texture, p, 0);
    let amount = params.x;
    if (amount <= 0.0) {
        return src;
    }
    var c = cas(p, params.y);
    c = vibrance(c, params.z);
    c = curve(c, params.w);
    c *= WARM;
    let q = (in.uv - vec2<f32>(0.5)) * 2.0;
    c *= 1.0 - VIGNETTE * clamp(dot(q, q) - 0.5, 0.0, 1.0);
    return vec4<f32>(mix(src.rgb, clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), amount), src.a);
}
