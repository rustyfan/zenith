struct Root { float4 color0; float4 color1; float depth; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
[[vk::constant_id(0)]] const float depthScale = 2.0;
[[vk::constant_id(1)]] const float colorScale = 2.0;
[shader("vertex")]
float4 vertexMain(uint vertex : SV_VulkanVertexID) : SV_Position
{
    float2 positions[3] = { float2(-1,-1),float2(3,-1),float2(-1,3) };
    return float4(positions[vertex],root.depth * depthScale,1);
}
struct Colors { float4 a : SV_Target0; float4 b : SV_Target1; };
[shader("fragment")]
Colors fragmentMain()
{
    Colors result; result.a = root.color0 * colorScale; result.b = root.color1 * colorScale; return result;
}
