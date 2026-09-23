import neural_material_common;
import brdf;
import tonemapping;

struct Root {
    float4* material;
    uint width;
    uint height;
    uint atlas_width;
    uint atlas_height;
    float rotation;
    float exposure;
    uint error_view;
    uint flat_view;
    uint encode_srgb;
    uint padding;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

float4 texel(int2 p, bool neural) {
    int2 size = int2(root.atlas_width, root.atlas_height);
    p = (p % size + size) % size;
    return neural ? root.material[p.y * size.x + p.x] : reference_material((float2(p) + 0.5) / float2(size));
}

float4 material(float2 uv, bool neural) {
    float2 p = frac(uv) * float2(root.atlas_width, root.atlas_height) - 0.5;
    int2 i = int2(floor(p));
    float2 f = frac(p);
    return lerp(lerp(texel(i, neural), texel(i + int2(1, 0), neural), f.x),
        lerp(texel(i + int2(0, 1), neural), texel(i + int2(1, 1), neural), f.x), f.y);
}

float3 shade(float3 n, float4 surface) {
    float3 v = float3(0, 0, 1);
    float3 color = surface.rgb * (0.12 + 0.18 * saturate(n.y));
    float3 directions[3] = { normalize(float3(-0.6, 0.8, 1.2)), normalize(float3(0.9, 0.3, 0.5)), normalize(float3(-0.2, -0.8, 0.6)) };
    float3 radiance[3] = { float3(3.5, 3.1, 2.6), float3(0.6, 1.1, 1.5), float3(0.35, 0.4, 0.45) };
    float alpha = max(surface.a * surface.a, 0.025);
    for (uint i = 0; i < 3; i++) {
        float3 l = directions[i];
        float3 h = normalize(v + l);
        float3 f = fresnel_schlick_appro(float3(0.04), l, h);
        float3 specular = f * ndf_ggx(n, h, alpha * alpha) * vis_smith_joint(v, l, n, alpha);
        color += ((1.0 - f) * diffuse_lambert(surface.rgb) + specular) * saturate(dot(n, l)) * radiance[i];
    }
    return aces_film(color * root.exposure);
}

[shader("fragment")]
float4 main(float4 position : SV_Position) : SV_Target {
    float2 resolution = float2(root.width, root.height);
    float2 screen = position.xy / resolution;
    bool right = screen.x >= 0.5;
    float radius = min(resolution.x * 0.205, resolution.y * 0.32);
    float2 center = float2(right ? 0.75 : 0.25, 0.57) * resolution;
    float2 p = (position.xy - center) / radius;
    p.y = -p.y;
    float3 color = lerp(float3(0.025, 0.037, 0.055), float3(0.075, 0.10, 0.13), 1.0 - screen.y);
    float shadow = exp(-dot(p * float2(0.9, 4.0) + float2(0.0, 4.45), p * float2(0.9, 4.0) + float2(0.0, 4.45)));
    color *= 1.0 - 0.65 * shadow;
    bool inside = root.flat_view != 0 ? max(abs(p.x), abs(p.y)) < 1.0 : dot(p, p) < 1.0;
    if (inside) {
        float3 n = root.flat_view != 0 ? float3(0, 0, 1) : float3(p, sqrt(max(1.0 - dot(p, p), 0.0)));
        float2 uv = root.flat_view != 0 ? p * float2(0.5, -0.5) + 0.5
            : float2(atan2(n.x, n.z) / MATERIAL_TAU + 0.5 + root.rotation, acos(clamp(n.y, -1.0, 1.0)) / 3.141592653589793);
        float4 value = material(uv, right);
        if (right && root.error_view != 0) {
            float4 difference = abs(value - material(uv, false));
            float error = max(max(difference.r, difference.g), max(difference.b, difference.a));
            color = lerp(float3(0.01, 0.025, 0.07), float3(1.0, 0.24, 0.035), saturate(error * 12.0));
        } else {
            color = root.flat_view != 0 ? value.rgb : shade(n, value);
        }
    }
    if (abs(position.x - resolution.x * 0.5) < 0.5) { color = float3(0.12, 0.17, 0.22); }
    if (root.encode_srgb != 0) { color = linear_to_srgb(max(color, 0.0)); }
    return float4(color, 1.0);
}
