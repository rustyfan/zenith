import zenith_bindings;
import brdf.hair_marschner;
import brdf.ibl_sampling;
import common.math_constants;

func shade_hair_direct(HairParameters hair, float3 tangent, float3 v, float3 l,
    float3 radiance, uint lobes, uint scattering_model, uint quality) -> float3 {
    return evaluate_hair(hair, tangent, v, l, lobes, scattering_model, quality) * radiance;
}
func shade_hair_environment(HairParameters hair, float3 tangent, float3 v,
    tex skybox, sampler sampler_id, uint samples, bool furnace,
    uint lobes, uint scattering_model, uint quality) -> float3 {
    TextureCube<float4> sky = ResourceDescriptorHeap[NonUniformResourceIndex(skybox)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(sampler_id)];
    var environment = float3(0.0);
    for (uint i = 0; i < samples; i++) {
        let u = hammersley(i, samples);
        let z = 1.0 - 2.0 * (float(i) + 0.5) / float(samples);
        let r = sqrt(max(0.0, 1.0 - z * z));
        let direction = float3(r * cos(2.0 * PI * u.y), r * sin(2.0 * PI * u.y), z);
        let incoming = furnace ? float3(1.0) : sky.SampleLevel(sampler, direction, 0).rgb;
        environment += incoming * evaluate_hair(hair, tangent, v, direction,
            lobes, scattering_model, quality);
    }
    return environment * (4.0 * PI / float(samples));
}
