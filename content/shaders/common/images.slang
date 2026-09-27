float4 sample_image(uint image_index, uint sampler_index, float2 uv) {
    Texture2D<float4> image = ResourceDescriptorHeap[NonUniformResourceIndex(image_index)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(sampler_index)];
    return image.Sample(sampler, uv);
}
