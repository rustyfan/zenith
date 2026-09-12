struct Root { uint* records; uint* count; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
[[vk::constant_id(0)]] const uint drawCount = 1;
[shader("compute")]
[numthreads(1, 1, 1)]
void generate()
{
    root.count[0] = drawCount;
    for (uint i = 0; i < drawCount; ++i)
    {
        root.records[5 * i + 0] = 6;
        root.records[5 * i + 1] = 1;
        root.records[5 * i + 2] = 0;
        root.records[5 * i + 3] = 0;
        root.records[5 * i + 4] = 0;
    }
}
