struct Root { uint background; uint accumulation; uint revealage; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
[shader("fragment")]
float4 main(float4 position : SV_Position) : SV_Target {
    let pixel = int3(int2(position.xy), 0);
    Texture2D<float4> background = ResourceDescriptorHeap[root.background];
    Texture2D<float4> accumulation = ResourceDescriptorHeap[root.accumulation];
    Texture2D<float> revealage = ResourceDescriptorHeap[root.revealage];
    let sum = accumulation.Load(pixel);
    let transmission = exp(-revealage.Load(pixel));
    let hair = sum.rgb / max(sum.a, 1e-8);
    return float4(lerp(hair, background.Load(pixel).rgb, transmission), 1.0);
}
