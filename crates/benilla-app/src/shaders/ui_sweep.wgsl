// Radial cooldown sweep over an ability icon: a dark pie anchored at 12 o'clock, sweeping
// clockwise, that shrinks as the cooldown completes. `sweep` is the remaining fraction —
// 1.0 is fully dark (just cast), 0.0 is clear (ready). Drawn over the icon with standard
// premultiplied alpha, so the icon darkens under it.
#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0) var<uniform> sweep: f32;

const TAU: f32 = 6.283185307179586;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let p = in.uv - vec2<f32>(0.5, 0.5);
    // Clockwise angle from 12 o'clock, in 0..TAU.
    var ang = atan2(p.x, -p.y);
    ang = select(ang, ang + TAU, ang < 0.0);
    // Positive inside the still-on-cooldown pie; the smoothstep is a soft edge just inside the
    // sweep line so the boundary does not alias.
    let d = sweep * TAU - ang;
    let alpha = smoothstep(0.0, 0.05, d) * 0.55;
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}
