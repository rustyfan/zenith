import zenith_bindings;
import brdf.brdf;
import brdf.ibl_sampling;
import brdf.opaque;
import common.math_constants;

struct TestCase { float4 material; float4 geometry; };
struct Root {
    TestCase* cases;
    float4* output;
    tex directional;
    tex average;
    sampler sampler;
    uint samples;
};
#include "zenith_root.slang"
groupshared float3 integrals[64];
groupshared float4 sampling_integrals[64];

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
    let bxdf = OpaqueBxdf.new(n, v, roughness, f0, diffuse, view_integral, average);
    let energy = bxdf.energy;
    let sampling_normal = normalize(float3(0.3, -0.4, -0.5));
    let sampling_bxdf = OpaqueBxdf.new(sampling_normal, sampling_normal, 1.0, f0, diffuse, view_integral, average);
    float3 integral = 0.0;
    float4 sampling_integral = 0.0;
    for (uint i = lane; i < root.samples; i += 64) {
        let u = hammersley(i / 2, root.samples / 2);
        let sample = bxdf.sample(float3(u, (i & 1) == 0 ? 1.0 : 0.0));
        let l = sample.direction;
        if (sample.pdf > 0.0) {
            let light_integral = lut.SampleLevel(sampler, brdf_lut_uv(l.z, roughness), 0);
            integral += bxdf.eval(l, light_integral) * (l.z / sample.pdf);
        }
        if (roughness == 1.0 && nov == 1.0) {
            let sample = sampling_bxdf.sample(float3(u, (i & 1) == 0 ? 1.0 : 0.0));
            if (sample.pdf > 0.0) {
                let cosine = dot(sampling_normal, sample.direction);
                sampling_integral.xyz += float3(1.0, cosine, cosine * cosine);
                let expected_pdf = (0.5 * cosine + 0.125) * ONE_OVER_PI;
                sampling_integral.w += !all(isfinite(sample.direction)) || !isfinite(sample.pdf)
                    || cosine <= 0.0 || abs(dot(sample.direction, sample.direction) - 1.0) > 1e-5
                    || abs(sample.pdf - expected_pdf) > 1e-5 ? 1.0 : 0.0;
            } else {
                sampling_integral.w += any(sample.direction != 0.0) || sample.pdf != 0.0 ? 1.0 : 0.0;
            }
        }
    }
    integrals[lane] = integral / float(root.samples);
    sampling_integrals[lane] = sampling_integral / float(root.samples);
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0) {
        integral = 0.0;
        sampling_integral = 0.0;
        for (uint j = 0; j < 64; j++) {
            integral += integrals[j];
            sampling_integral += sampling_integrals[j];
        }
        let nol = test.geometry.z;
        let tangent = sqrt(saturate(1.0 - nol * nol));
        let l = float3(tangent * cos(test.geometry.w), tangent * sin(test.geometry.w), nol);
        let light_integral = lut.SampleLevel(sampler, brdf_lut_uv(nol, roughness), 0);
        let reverse_energy = MicrofacetEnergy.new(f0, diffuse, light_integral, average);
        root.output[group.x * 6] = float4(integral, 1.0);
        root.output[group.x * 6 + 1] = float4(energy.environment(float3(1.0), float3(1.0), 1.0), 1.0);
        root.output[group.x * 6 + 2] = float4(bxdf.eval(l, light_integral), 1.0);
        root.output[group.x * 6 + 3] = float4(evaluate_microfacet_brdf(n, l, v, alpha, f0, reverse_energy, view_integral), 1.0);
        let smooth = MicrofacetEnergy.new(f0, diffuse, float2(1.0, 0.0), float2(1.0, 0.0));
        root.output[group.x * 6 + 4] = float4(smooth.evaluate(f0, float2(1.0, 0.0)), 1.0);
        root.output[group.x * 6 + 5] = sampling_integral;
    }
}
