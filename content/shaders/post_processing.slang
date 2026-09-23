import tonemapping;

struct Root {
    uint hdr_color;
    float exposure;
    uint encode_srgb;
    uint bypass_tonemapping;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("fragment")]
float4 main(float4 position : SV_Position) : SV_Target {
    Texture2D<float4> hdr_color = ResourceDescriptorHeap[NonUniformResourceIndex(root.hdr_color)];
    let radiance = max(hdr_color.Load(int3(int2(position.xy), 0)).rgb, 0.0);
    if (root.bypass_tonemapping != 0) {
        return float4(root.encode_srgb != 0 ? linear_to_srgb(radiance) : radiance, 1.0);
    }
    // Bound the ACES fit to avoid overflow in its quadratic terms.
    let exposed = root.exposure == 0.0 ? float3(0.0) : min(radiance * root.exposure, 1e4);
    let mapped = aces_film(exposed);
    return float4(root.encode_srgb != 0 ? linear_to_srgb(mapped) : mapped, 1.0);
}
