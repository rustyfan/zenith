import zenith_bindings;
float4 sample_image(tex image_index, sampler sampler_index, float2 uv) {
    Texture2D<float4> image = ResourceDescriptorHeap[NonUniformResourceIndex(image_index)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(sampler_index)];
    return image.Sample(sampler, uv);
}
