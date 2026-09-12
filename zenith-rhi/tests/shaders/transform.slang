struct Root
{
    uint* input;
    uint* output;
    uint count;
    uint multiplier;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

[shader("compute")]
[numthreads(64, 1, 1)]
void transform(uint3 id : SV_DispatchThreadID)
{
    if (id.x < root.count)
        root.output[id.x] = root.input[id.x] * root.multiplier + id.x;
}
