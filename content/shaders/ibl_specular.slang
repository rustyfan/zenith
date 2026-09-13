import ibl_sampling;
import brdf;
import math_constants;

struct Root {
    uint skybox;
    uint sampler;
    uint output;
    uint size;
    float roughness;
    float source_size;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= root.size) || id.z >= 6) { return; }
    let n = cubemap_direction(id.z, (float2(id.xy) + 0.5) / float(root.size));
    TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
    RWTexture2DArray<float4> output = ResourceDescriptorHeap[NonUniformResourceIndex(root.output)];
    if (root.roughness == 0.0) {
        output[id] = float4(skybox.SampleLevel(sampler, n, 0).rgb, 1.0);
        return;
    }
    let basis = build_orthonormal_basis(n);
    let alpha = root.roughness * root.roughness;
    const uint samples = 512;
    float3 radiance = 0.0;
    float weight = 0.0;
    let texel_solid_angle = 4.0 * PI / (6.0 * root.source_size * root.source_size);
    for (uint i = 0; i < samples; i++) {
        let h = normalize(mul(basis, sample_ggx(hammersley(i, samples), alpha)));
        let l = reflect(-n, h);
        let nol = saturate(dot(n, l));
        if (nol > 0.0) {
            // V=N: GGX reflection PDF simplifies to D/4.
            let pdf = max(ndf_ggx(n, h, alpha * alpha) * 0.25, 1e-7);
            let lod = max(0.5 * log2(1.0 / (float(samples) * pdf * texel_solid_angle)), 0.0);
            radiance += skybox.SampleLevel(sampler, l, lod).rgb * nol;
            weight += nol;
        }
    }
    output[id] = float4(radiance / max(weight, 1e-7), 1.0);
}
