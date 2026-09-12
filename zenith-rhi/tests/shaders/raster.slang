struct Vertex { float2 position; float2 padding; float4 color; };
struct Root { Vertex* vertices; float4 tint; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; float4 color : COLOR0; };
[shader("vertex")]
Varying vertexMain(uint id : SV_VulkanVertexID)
{
    Varying output;
    output.position = float4(root.vertices[id].position, 0.5, 1.0);
    output.color = root.vertices[id].color;
    return output;
}
[shader("fragment")]
float4 fragmentMain(Varying input) : SV_Target0
{
    return input.color * root.tint;
}
