struct Root {
    uint images[9];
    uint samplers[2];
    uint output;
    float4* result;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    SamplerState varying_sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.samplers[(id.x >> 1) & 1])];
    SamplerState sampler = SamplerDescriptorHeap[root.samplers[0]];
    Texture2D<float4> varying_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.images[2 + (id.x & 1)])];
    Texture1D<float4> image1d = ResourceDescriptorHeap[root.images[0]];
    Texture1DArray<float4> array1d = ResourceDescriptorHeap[root.images[1]];
    Texture2DArray<float4> array2d = ResourceDescriptorHeap[root.images[4]];
    Texture3D<float4> image3d = ResourceDescriptorHeap[root.images[5]];
    TextureCube<float4> cube = ResourceDescriptorHeap[root.images[6]];
    TextureCubeArray<float4> cube_array = ResourceDescriptorHeap[root.images[7]];
    Texture2D<float> depth = ResourceDescriptorHeap[root.images[8]];
    RWTexture2D<float4> output = ResourceDescriptorHeap[root.output];
    float4 values[8] = {
        varying_image.SampleLevel(varying_sampler, float2(1.5, 0.5), 0),
        image1d.SampleLevel(sampler, 0.5, 0),
        array1d.SampleLevel(sampler, float2(0.5, 1), 0),
        array2d.SampleLevel(sampler, float3(0.5, 0.5, 1), 0),
        image3d.SampleLevel(sampler, float3(0.5), 0),
        cube.SampleLevel(sampler, float3(1, 0, 0), 0),
        cube_array.SampleLevel(sampler, float4(1, 0, 0, 1), 0),
        float4(depth.SampleLevel(sampler, float2(0.5), 0)),
    };
    for (uint i = 0; i < 8; ++i) {
        root.result[id.x * 8 + i] = values[i];
        output[uint2(id.x, i)] = values[i];
    }
}
