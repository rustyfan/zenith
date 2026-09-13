import view;
import shading_data;
import ibl_sampling;
import brdf;
import screen_quad;
import sh;
import debug;
import tonemapping;

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
    float exposure;
    float max_mip;
    uint encode_srgb;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

struct PSOutput { float4 output : SV_Target; };

func display_color(float3 radiance) -> float3 {
    let mapped = aces_film(min(max(radiance, 0.0) * root.exposure, 1e4));
    return root.encode_srgb != 0 ? linear_to_srgb(mapped) : mapped;
}

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
    if (depth == 0.0) {
        TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
        let radiance = (root.debug_mode & DebugMode.DiffuseSH) != 0
            ? evaluate_diffuse_sh(root.sh, view_dir)
            : skybox.SampleLevel(sampler, view_dir, 0).rgb;
        output.output = float4(display_color(radiance * root.sky_intensity), 1.0);
        return output;
    }

    Texture2D<float4> base_color = ResourceDescriptorHeap[NonUniformResourceIndex(root.base_color)];
    Texture2D<float4> normal_mra = ResourceDescriptorHeap[NonUniformResourceIndex(root.normal_mra)];
    let base = base_color.Load(pixel).rgb;
    float3 n;
    float metallic, roughness;
    decode_normal_mra(normal_mra.Load(pixel), n, metallic, roughness);
    metallic = saturate(metallic);
    roughness = clamp(roughness, MIN_ROUGHNESS, 1.0);
    let v = -view_dir;
    let nov = saturate(dot(n, v));
    let specular_color = lerp(float3(0.04), base, metallic);
    let diffuse_color = (1.0 - metallic) * base;
    let alpha = roughness * roughness;

    float3 direct = 0.0;
    let l = root.direction_to_light.xyz;
    let nol = saturate(dot(n, l));
    if (nol > 0.0 && nov > 0.0 && dot(l + v, l + v) > 1e-8) {
        let h = normalize(l + v);
        let f = fresnel_schlick_appro(specular_color, l, h);
        let specular = f * ndf_ggx(n, h, alpha * alpha) * vis_smith_joint(v, l, n, alpha);
        direct = ((1.0 - f) * diffuse_lambert(diffuse_color) + specular) * root.light_radiance.xyz * nol;
    }

    let ambient_f = fresnel_schlick_roughness_appro(specular_color, n, v, roughness);
    let diffuse_ibl = (1.0 - ambient_f) * diffuse_color * evaluate_diffuse_sh(root.sh, n);
    TextureCube<float4> environment = ResourceDescriptorHeap[NonUniformResourceIndex(root.specular)];
    Texture2D<float2> brdf_lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.brdf_lut)];
    let integrated = brdf_lut.SampleLevel(sampler, float2(nov, roughness), 0);
    let reflected = environment.SampleLevel(sampler, reflect(-v, n), roughness * root.max_mip).rgb;
    let specular_ibl = reflected * (specular_color * integrated.x + integrated.y);
    output.output = float4(display_color(direct + (diffuse_ibl + specular_ibl) * root.sky_intensity), 1.0);
    return output;
}
