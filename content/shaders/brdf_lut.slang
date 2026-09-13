import ibl_sampling;
import brdf;

struct Root { uint output; uint size; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (any(id.xy >= root.size)) { return; }
    let uv = (float2(id.xy) + 0.5) / float(root.size);
    let nov = uv.x;
    let roughness = max(uv.y, MIN_ROUGHNESS);
    let alpha = roughness * roughness;
    let v = float3(sqrt(1.0 - nov * nov), 0.0, nov);
    float2 integrated = 0.0;
    const uint samples = 1024;
    for (uint i = 0; i < samples; i++) {
        let h = sample_ggx(hammersley(i, samples), alpha);
        let voh = saturate(dot(v, h));
        let l = 2.0 * voh * h - v;
        let nol = saturate(l.z);
        if (nol > 0.0) {
            // (D * V * NoL) / (D * NoH / (4 * VoH)).
            let weight = 4.0 * vis_smith_ggx(nov, nol, alpha) * nol * voh / max(h.z, 1e-7);
            let fc = pow(1.0 - voh, 5.0);
            integrated += float2(1.0 - fc, fc) * weight;
        }
    }
    RWTexture2D<float2> output = ResourceDescriptorHeap[NonUniformResourceIndex(root.output)];
    output[id.xy] = integrated / float(samples);
}
