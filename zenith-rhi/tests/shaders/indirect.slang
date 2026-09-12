struct DrawData { float offset; float4 tint; };
struct Root { float2* vertices; DrawData* draws; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; nointerpolation uint draw : TEXCOORD0; };
[shader("vertex")]
Varying vertexMain(uint vertex : SV_VulkanVertexID, uint draw : SV_DrawIndex)
{
    Varying output;
    float2 position = root.vertices[vertex];
    output.position = float4(position.x * 0.5 + root.draws[draw].offset, position.y, 0.5, 1.0);
    output.draw = draw;
    return output;
}
[shader("fragment")]
float4 fragmentMain(Varying input) : SV_Target0 { return root.draws[input.draw].tint; }
