struct Root { uint image; uint samplerIndex; uint outputImage; uint padding; float4* result; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("compute")]
[numthreads(1, 1, 1)]
void sampleImage(uint3 id : SV_DispatchThreadID)
{
    Texture2D<float4> image = ResourceDescriptorHeap[NonUniformResourceIndex(root.image)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.samplerIndex)];
    RWTexture2D<float4> output = ResourceDescriptorHeap[NonUniformResourceIndex(root.outputImage)];
    float4 value = image.SampleLevel(sampler, float2(0.5, 0.5), 0);
    output[id.xy] = value;
    root.result[id.x] = value;
}
