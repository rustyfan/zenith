struct Vertex { float2 position; float2 uv; uint color; };
struct Root {
    Vertex* vertices;
    float2 screen_size;
    uint image;
    uint sampler_index;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

[shader("vertex")]
Varying vsmain(uint id : SV_VulkanVertexID) {
    let vertex = root.vertices[id];
    Varying output;
    output.position = float4(2.0 * vertex.position / root.screen_size - 1.0, 0.0, 1.0);
    output.uv = vertex.uv;
    output.color = float4(vertex.color & 255, (vertex.color >> 8) & 255,
        (vertex.color >> 16) & 255, vertex.color >> 24) / 255.0;
    return output;
}

[shader("fragment")]
float4 psmain(Varying input) : SV_Target {
    Texture2D<float4> image = ResourceDescriptorHeap[NonUniformResourceIndex(root.image)];
    SamplerState image_sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler_index)];
    return input.color * image.Sample(image_sampler, input.uv);
}

[shader("vertex")]
float4 composite_vs(uint id : SV_VulkanVertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 composite_ps(float4 position : SV_Position) : SV_Target {
    Texture2D<float4> overlay = ResourceDescriptorHeap[NonUniformResourceIndex(root.image)];
    float4 color = overlay.Load(int3(int2(position.xy), 0));
    // Decode straight sRGB before restoring premultiplication for linear scene compositing.
    float weight = color.a > 0.0 ? color.a : 1.0;
    float3 straight = color.rgb / weight;
    float3 linear = select(straight <= 0.04045, straight / 12.92,
        pow((straight + 0.055) / 1.055, 2.4));
    return float4(linear * weight, color.a);
}
