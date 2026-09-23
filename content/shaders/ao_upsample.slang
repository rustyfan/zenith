struct Root {
    uint ao;
    uint depth;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("fragment")]
float main(float4 position : SV_Position) : SV_Target {
    Texture2D<float> depth_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
    let pixel = int2(position.xy);
    let depth = depth_image.Load(int3(pixel, 0));
    if (depth == 0.0) {
        return 1.0;
    }
    Texture2D<float> ao_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.ao)];
    uint width, height;
    ao_image.GetDimensions(width, height);
    let half_pixel = float2(pixel) * 0.5;
    let base = int2(half_pixel);
    let fraction = frac(half_pixel);
    float total = 0.0;
    float weight_sum = 0.0;
    [unroll]
    for (int y = 0; y < 2; ++y) {
        [unroll]
        for (int x = 0; x < 2; ++x) {
            let sample_pixel = min(base + int2(x, y), int2(width, height) - 1);
            let sample_depth = depth_image.Load(int3(sample_pixel * 2, 0));
            if (sample_depth == 0.0) {
                continue;
            }
            // With infinite reverse-Z, depth / sample_depth equals sample_z / center_z.
            let depth_error = abs(depth / sample_depth - 1.0);
            // A 1% relative-depth Gaussian, truncated at 2% to reject discontinuities.
            if (depth_error > 0.02) {
                continue;
            }
            let spatial = (x == 0 ? 1.0 - fraction.x : fraction.x)
                * (y == 0 ? 1.0 - fraction.y : fraction.y);
            let weight = spatial * exp2(-10000.0 * depth_error * depth_error);
            total += ao_image.Load(int3(sample_pixel, 0)) * weight;
            weight_sum += weight;
        }
    }
    return weight_sum > 0.0 ? saturate(total / weight_sum) : 1.0;
}
