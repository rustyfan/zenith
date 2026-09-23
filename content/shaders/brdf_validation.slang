import brdf;
import ibl_sampling;
import math_constants;

struct TestCase { float4 material; float4 geometry; };
struct Root {
    TestCase* cases;
    float4* output;
    uint directional;
    uint average;
    uint sampler;
    uint samples;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
groupshared float3 integrals[64];

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint3 group : SV_GroupID, uint lane : SV_GroupIndex) {
    let test = root.cases[group.x];
    let f0 = lerp(float3(0.04), test.material.xyz, test.material.w);
    let diffuse = (1.0 - test.material.w) * test.material.xyz;
    let roughness = max(test.geometry.x, MIN_ROUGHNESS);
    let alpha = roughness * roughness;
    let nov = test.geometry.y;
    let v = float3(sqrt(saturate(1.0 - nov * nov)), 0.0, nov);
    let n = float3(0.0, 0.0, 1.0);
    Texture2D<float2> lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.directional)];
    Texture2D<float2> average_lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.average)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
    let average = average_lut.SampleLevel(sampler, float2(0.5, roughness), 0);
    let view_integral = lut.SampleLevel(sampler, brdf_lut_uv(nov, roughness), 0);
    let energy = MicrofacetEnergy.new(f0, diffuse, view_integral, average);
    float3 integral = 0.0;
    for (uint i = lane; i < root.samples; i += 64) {
        let u = hammersley(i / 2, root.samples / 2);
        float3 l;
        if ((i & 1) == 0) {
            let h = sample_ggx(u, alpha);
            l = reflect(-v, h);
        } else {
            let radius = sqrt(u.y);
            l = float3(radius * cos(2.0 * PI * u.x), radius * sin(2.0 * PI * u.x), sqrt(1.0 - u.y));
        }
        if (l.z > 0.0 && dot(l + v, l + v) > 1e-8) {
            let h = normalize(l + v);
            let pdf = 0.5 * (ndf_ggx(n, h, alpha * alpha) * h.z / (4.0 * dot(v, h)) + l.z * ONE_OVER_PI);
            let light_integral = lut.SampleLevel(sampler, brdf_lut_uv(l.z, roughness), 0);
            integral += evaluate_microfacet_brdf(n, v, l, alpha, f0, energy, light_integral) * (l.z / pdf);
        }
    }
    integrals[lane] = integral / float(root.samples);
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0) {
        integral = 0.0;
        for (uint j = 0; j < 64; j++) { integral += integrals[j]; }
        let nol = test.geometry.z;
        let tangent = sqrt(saturate(1.0 - nol * nol));
        let l = float3(tangent * cos(test.geometry.w), tangent * sin(test.geometry.w), nol);
        let light_integral = lut.SampleLevel(sampler, brdf_lut_uv(nol, roughness), 0);
        let reverse_energy = MicrofacetEnergy.new(f0, diffuse, light_integral, average);
        root.output[group.x * 5] = float4(integral, 1.0);
        root.output[group.x * 5 + 1] = float4(energy.environment(float3(1.0), float3(1.0), 1.0), 1.0);
        root.output[group.x * 5 + 2] = float4(evaluate_microfacet_brdf(n, v, l, alpha, f0, energy, light_integral), 1.0);
        root.output[group.x * 5 + 3] = float4(evaluate_microfacet_brdf(n, l, v, alpha, f0, reverse_energy, view_integral), 1.0);
        let smooth = MicrofacetEnergy.new(f0, diffuse, float2(1.0, 0.0), float2(1.0, 0.0));
        root.output[group.x * 5 + 4] = float4(smooth.evaluate(f0, float2(1.0, 0.0)), 1.0);
    }
}
