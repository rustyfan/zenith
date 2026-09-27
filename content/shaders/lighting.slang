import zenith_bindings;
import shading.surface;
import common.view;
import shading.shading_data;
import shading.opaque;
import shading.clearcoat;
import shading.shading_model;
import screen_quad;
import shading.sh;
import debug.debug;
import raytracing.ray_query;
import raytracing.hair_shadow;

[[vk::constant_id(0)]] const uint WHITE_FURNACE = 0;
[[vk::constant_id(1)]] const uint MULTIPLE_SCATTERING = 1;

struct Root {
    ViewData* view;
    float4* sh;
    tex base_color;
    tex normal_mra;
    tex depth;
    tex skybox;
    tex specular;
    tex brdf_lut;
    sampler linear_sampler;
    uint debug_mode;
    float4 direction_to_light;
    float4 light_radiance;
    float sky_intensity;
    float max_mip;
    uint64_t acceleration;
    float shadow_bias;
    float shadow_distance;
    uint shadows_enabled;
    uint global_illumination;
    tex brdf_average;
    tex coat;
    uint64_t hair_acceleration;
    float4* hair_materials;
    tex shading_model;
    uint padding;
};
#include "zenith_root.slang"

struct PSOutput { float4 output : SV_Target; };

[shader("fragment")]
PSOutput main(PSInput input) {
    let ndc_pos = float4((input.uv - 0.5) * float2(2.0, -2.0), 1.0, 1.0);
    let world_pos = mul(root.view->inv_view_proj, ndc_pos);
    let view_dir = normalize(world_pos.xyz / world_pos.w - root.view->position.xyz);
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.linear_sampler)];
    Texture2D<float> depth_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
    uint width, height;
    depth_image.GetDimensions(width, height);
    let pixel = int3(int2(input.uv * float2(width, height)), 0);
    let depth = depth_image.Load(pixel);

    PSOutput output;
    let furnace = WHITE_FURNACE != 0;
    Texture2D<float> global_illumination = ResourceDescriptorHeap[NonUniformResourceIndex(root.global_illumination)];
    let ao = global_illumination.Load(pixel);
    if (!furnace && (root.debug_mode & DebugMode.AmbientOcclusion) != 0) {
        output.output = float4(float3(ao), 1.0);
        return output;
    }
    if (depth == 0.0) {
        if (furnace) {
            output.output = 1.0;
            return output;
        }
        TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
        let radiance = (root.debug_mode & DebugMode.DiffuseSH) != 0
            ? evaluate_diffuse_sh(root.sh, view_dir)
            : skybox.SampleLevel(sampler, view_dir, 0).rgb;
        output.output = float4(radiance * root.sky_intensity, 1.0);
        return output;
    }

    Texture2D<float4> base_color = ResourceDescriptorHeap[NonUniformResourceIndex(root.base_color)];
    Texture2D<float4> normal_mra = ResourceDescriptorHeap[NonUniformResourceIndex(root.normal_mra)];
    let data = ShadingData.from_gbuffer(base_color.Load(pixel), normal_mra.Load(pixel));
    let base = data.basecolor;
    let n = data.normal;
    let metallic = data.metallic;
    let roughness = data.roughness;
    let v = -view_dir;
    SurfaceInput surface;
    surface.base = base;
    surface.n = n;
    surface.v = v;
    surface.metallic = metallic;
    surface.roughness = roughness;
    surface.ao = ao;
    SurfaceLighting lighting;
    lighting.sh = root.sh;
    lighting.brdf_lut = root.brdf_lut;
    lighting.brdf_average = root.brdf_average;
    lighting.specular = root.specular;
    lighting.linear_sampler = root.linear_sampler;
    lighting.debug_mode = root.debug_mode;
    lighting.direction_to_light = root.direction_to_light.xyz;
    lighting.light_radiance = root.light_radiance.xyz;
    lighting.max_mip = root.max_mip;
    lighting.furnace = furnace;
    lighting.multiple_scattering = MULTIPLE_SCATTERING != 0;
    Texture2D<uint> model_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.shading_model)];
    ShadingResult shaded;
    let model = model_image.Load(pixel);
    if ((root.debug_mode & DebugMode.ShadingModel) != 0) {
        output.output = float4(shading_model_color(model), 1.0);
        return output;
    }
    switch (model) {
        case uint(ShadingModel.Opaque):
            shaded = shade_opaque(surface, lighting);
            break;
        case uint(ShadingModel.ClearCoat): {
            Texture2D<float4> coat_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.coat)];
            shaded = shade_clearcoat(surface, lighting, coat_image.Load(pixel));
            break;
        }
        // Multilayer Hair is dispatched to the forward pass; this GBuffer has no strand parameters.
        case uint(ShadingModel.Hair):
        default:
            output.output = float4(1.0, 0.0, 1.0, 1.0);
            return output;
    }
    if (furnace) {
        output.output = float4(shaded.indirect, 1.0);
        return output;
    }
    var direct = shaded.direct;
    let l = root.direction_to_light.xyz;
    if (root.acceleration != 0 && root.shadows_enabled != 0 && any(direct > 0.0)) {
        let position = surface_position(root.view, input.uv, depth);
        let normal = surface_normal(root.view, depth_image, pixel.xy, int2(width, height), depth, position, v, n);
        direct *= ray_visibility(root.acceleration, position, normal, l, root.shadow_bias, root.shadow_distance);
    }
    if (root.hair_acceleration != 0 && root.shadows_enabled != 0) {
        let position = surface_position(root.view, input.uv, depth);
        direct *= hair_transmittance(root.hair_acceleration, root.hair_materials, position, l, root.shadow_bias, root.shadow_distance);
    }
    output.output = float4(direct + shaded.indirect * root.sky_intensity, 1.0);
    return output;
}
