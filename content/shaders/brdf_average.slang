struct Root { uint input; uint output; uint size; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= root.size) { return; }
    Texture2D<float2> directional = ResourceDescriptorHeap[NonUniformResourceIndex(root.input)];
    let step = 1.0 / float(root.size);
    let edge = 0.5 * step;
    float2 left = directional.Load(int3(0, id.x, 0));
    float2 average = left * (edge * edge * edge * edge);
    for (uint x = 0; x + 1 < root.size; x++) {
        // With x=sqrt(NoV), Eavg = integral(4*x^3*E(x) dx). Integrate each linear segment exactly.
        let a = (float(x) + 0.5) * step;
        let left_weight = step * (2.0 * a * a * a + step * (2.0 * a * a + step * (a + step / 5.0)));
        let right_weight = step * (2.0 * a * a * a + step * (4.0 * a * a + step * (3.0 * a + 4.0 * step / 5.0)));
        let right = directional.Load(int3(x + 1, id.x, 0));
        average += left * left_weight + right * right_weight;
        left = right;
    }
    average += left * (edge * (4.0 + edge * (-6.0 + edge * (4.0 - edge))));
    RWTexture2D<float2> output = ResourceDescriptorHeap[NonUniformResourceIndex(root.output)];
    output[uint2(0, id.x)] = average;
}
