import view;
import shading_data;
import ibl_sampling;
import brdf;
import screen_quad;
import sh;
import debug;
import ray_query;

[[vk::constant_id(0)]] const uint WHITE_FURNACE = 0;
[[vk::constant_id(1)]] const uint MULTIPLE_SCATTERING = 1;

struct Root {
    ViewData* view;
    float4* sh;
    uint base_color;
    uint normal_mra;
    uint depth;
    uint skybox;
    uint specular;
    uint brdf_lut;
    uint linear_sampler;
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
    uint brdf_average;
    uint padding;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

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
    let base = furnace ? float3(1.0) : base_color.Load(pixel).rgb;
    float3 n;
    float metallic, roughness;
    decode_normal_mra(normal_mra.Load(pixel), n, metallic, roughness);
    metallic = saturate(metallic);
    roughness = clamp(roughness, MIN_ROUGHNESS, 1.0);
    let v = -view_dir;
    let nov = saturate(dot(n, v));
    let specular_color = lerp(float3(0.04), base, metallic);
    let diffuse_color = furnace ? float3(1.0) : (1.0 - metallic) * base;
    let alpha = roughness * roughness;
    Texture2D<float2> brdf_lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.brdf_lut)];
    Texture2D<float2> brdf_average = ResourceDescriptorHeap[NonUniformResourceIndex(root.brdf_average)];
    let integrated = brdf_lut.SampleLevel(sampler, brdf_lut_uv(nov, roughness), 0);
    let average = brdf_average.SampleLevel(sampler, float2(0.5, roughness), 0);
    var energy = MicrofacetEnergy.new(specular_color, diffuse_color, integrated, average);
    if (MULTIPLE_SCATTERING == 0) {
        energy.multiple_scattering = 0.0;
    }
    if (furnace) {
        output.output = float4(energy.environment(float3(1.0), float3(1.0), 1.0), 1.0);
        return output;
    }

    float3 direct = 0.0;
    let l = root.direction_to_light.xyz;
    let nol = saturate(dot(n, l));
    if (nol > 0.0 && nov > 0.0 && dot(l + v, l + v) > 1e-8) {
        let light_integral = brdf_lut.SampleLevel(sampler, brdf_lut_uv(nol, roughness), 0);
        direct = evaluate_microfacet_brdf(n, v, l, alpha, specular_color, energy, light_integral)
            * root.light_radiance.xyz * nol;
        if (root.acceleration != 0 && root.shadows_enabled != 0 && any(root.light_radiance.xyz > 0.0)) {
            let position = surface_position(root.view, input.uv, depth);
            let normal = surface_normal(root.view, depth_image, pixel.xy, int2(width, height), depth, position, v, n);
            direct *= ray_visibility(root.acceleration, position, normal, l, root.shadow_bias, root.shadow_distance);
        }
    }

    TextureCube<float4> environment = ResourceDescriptorHeap[NonUniformResourceIndex(root.specular)];
    let reflected = environment.SampleLevel(sampler, reflect(-v, n), roughness * root.max_mip).rgb;
    let ibl = energy.environment(reflected, evaluate_diffuse_sh(root.sh, n), ao);
    output.output = float4(direct + ibl * root.sky_intensity, 1.0);
    return output;
}
