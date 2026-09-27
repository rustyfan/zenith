import zenith_bindings;
import brdf.brdf;
import brdf.ibl_sampling;

struct SurfaceInput {
    float3 base;
    float3 n;
    float3 v;
    float metallic;
    float roughness;
    float ao;
};
struct SurfaceLighting {
    float4* sh;
    tex brdf_lut;
    tex brdf_average;
    tex specular;
    sampler linear_sampler;
    uint debug_mode;
    float3 direction_to_light;
    float3 light_radiance;
    float max_mip;
    bool furnace;
    bool multiple_scattering;
};
struct ShadingResult { float3 direct; float3 indirect; };
func surface_integral(SurfaceLighting lighting, float cosine, float roughness) -> float2 {
    Texture2D<float2> lut = ResourceDescriptorHeap[NonUniformResourceIndex(lighting.brdf_lut)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(lighting.linear_sampler)];
    return lut.SampleLevel(sampler, brdf_lut_uv(cosine, roughness), 0);
}
func surface_average(SurfaceLighting lighting, float roughness) -> float2 {
    Texture2D<float2> lut = ResourceDescriptorHeap[NonUniformResourceIndex(lighting.brdf_average)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(lighting.linear_sampler)];
    return lut.SampleLevel(sampler, float2(0.5, roughness), 0);
}
func surface_reflection(SurfaceLighting lighting, float3 n, float3 v, float roughness) -> float3 {
    if (lighting.furnace) { return 1.0; }
    TextureCube<float4> environment = ResourceDescriptorHeap[NonUniformResourceIndex(lighting.specular)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(lighting.linear_sampler)];
    return environment.SampleLevel(sampler, reflect(-v, n), roughness * lighting.max_mip).rgb;
}
