import common.view;
import debug.debug;
import common.math_constants;
import brdf.brdf;
import brdf.ibl_sampling;
import brdf.hair_marschner;
import raytracing.ray_query;
import raytracing.hair_shadow;

struct Vertex { float3 position; float3 normal; float2 tex_coord; float4 tangent; };
struct Root {
    ViewData* view;
    Vertex* vertices;
    float4x4 model;
    HairParameters hair;
    float4 direction;
    float4 radiance;
    uint64_t opaque_acceleration;
    uint64_t hair_acceleration;
    float4* shadow_materials;
    uint skybox;
    uint sampler;
    float sky_intensity;
    float bias;
    float distance;
    uint lobes;
    uint scattering_model;
    uint quality;
    uint environment_samples;
    uint debug_mode;
    uint depth;
    uint paired_ribbons;
    float2 viewport;
};
#include "zenith_root.slang"
struct Varying {
    float4 position : SV_Position;
    float3 world : TEXCOORD0;
    float3 tangent : TEXCOORD1;
    float2 uv : TEXCOORD2;
    noperspective float2 footprint : TEXCOORD3;
};
[shader("vertex")]
Varying vsmain(uint id : SV_VulkanVertexID) {
    let vertex = root.vertices[id];
    Varying output;
    output.world = mul(root.model, float4(vertex.position, 1.0)).xyz;
    output.position = mul(root.view->view_proj, float4(output.world, 1.0));
    output.tangent = normalize(mul(root.model, float4(vertex.tangent.xyz, 0.0)).xyz);
    output.uv = vertex.tex_coord;
    output.footprint = float2(0.0, 1.0);
    if (root.paired_ribbons != 0) {
        let other = mul(root.model, float4(root.vertices[id ^ 1].position, 1.0)).xyz;
        let radius = length(output.world - other) * 0.5;
        output.world = (output.world + other) * 0.5;
        let view = normalize(root.view->position.xyz - output.world);
        var side = cross(output.tangent, view);
        if (dot(side, side) < 1e-8) { side = cross(output.tangent, float3(1, 0, 0)); }
        if (dot(side, side) < 1e-8) { side = cross(output.tangent, float3(0, 1, 0)); }
        side = normalize(side);
        let center = mul(root.view->view_proj, float4(output.world, 1.0));
        let edge = mul(root.view->view_proj, float4(output.world + side * radius, 1.0));
        let delta = (edge.xy / edge.w - center.xy / center.w) * root.viewport * 0.5;
        let pixel_radius = length(delta);
        let direction = delta / max(pixel_radius, 1e-8);
        // Rasterize the pixel filter's support, then integrate the physical strand coverage.
        let offset = (vertex.tex_coord.x * 2.0 - 1.0) * (pixel_radius + 0.5);
        output.position = center;
        output.position.xy += direction * offset * 2.0 / root.viewport * center.w;
        output.footprint = float2(offset, pixel_radius);
    }
    return output;
}
struct Output { float4 accumulation : SV_Target0; float revealage : SV_Target1; };
[shader("fragment")]
Output psmain(Varying input) {
    Texture2D<float> depth = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
    let coverage = root.paired_ribbons != 0
        ? saturate(min(input.footprint.x + input.footprint.y, 0.5)
            - max(input.footprint.x - input.footprint.y, -0.5))
        : saturate(min(input.uv.x, 1.0 - input.uv.x) / max(fwidth(input.uv.x), 1e-5) + 0.5);
    if (input.position.z < depth.Load(int3(int2(input.position.xy), 0))) {
        Output empty;
        empty.accumulation = 0.0;
        empty.revealage = 0.0;
        return empty;
    }
    let tangent = normalize(input.tangent);
    let v = normalize(root.view->position.xyz - input.world);
    let l = root.direction.xyz;
    let furnace = (root.debug_mode & DebugMode.WhiteFurnace) != 0;
    var color = float3(0.0);
    if (!furnace && any(root.radiance.xyz > 0.0)) {
        color = evaluate_hair(root.hair, tangent, v, l, root.lobes, root.scattering_model, root.quality) * root.radiance.xyz;
        if (root.opaque_acceleration != 0) {
            color *= ray_visibility(root.opaque_acceleration, input.world, l, l, root.bias, root.distance);
        }
        color *= hair_transmittance(root.hair_acceleration, root.shadow_materials, input.world,
            l, root.bias, root.distance);
    }
    if (furnace || root.sky_intensity > 0.0) {
        TextureCube<float4> sky = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
        SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
        var environment = float3(0.0);
        for (uint i = 0; i < root.environment_samples; i++) {
            let u = hammersley(i, root.environment_samples);
            let z = 1.0 - 2.0 * (float(i) + 0.5) / float(root.environment_samples);
            let r = sqrt(max(0.0, 1.0 - z * z));
            let direction = float3(r * cos(2.0 * PI * u.y), r * sin(2.0 * PI * u.y), z);
            let incoming = furnace ? float3(1.0) : sky.SampleLevel(sampler, direction, 0).rgb;
            environment += incoming * evaluate_hair(root.hair, tangent, v, direction,
                root.lobes, root.scattering_model, root.quality);
        }
        color += environment * (4.0 * PI / float(root.environment_samples)) * (furnace ? 1.0 : root.sky_intensity);
    }
    if ((root.debug_mode & DebugMode.HairTangent) != 0) { color = tangent * 0.5 + 0.5; }
    let alpha = coverage * root.hair.opacity;
    let weight = clamp(0.01 + 1000.0 * pow(input.position.z, 3.0), 0.01, 10.0);
    Output output;
    output.accumulation = float4(color * alpha, alpha) * weight;
    output.revealage = -log(max(1.0 - alpha, 1e-6));
    return output;
}
