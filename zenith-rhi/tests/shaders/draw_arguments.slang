struct Root { float2* vertices; uint firstInstance; float scale; float4 firstColor; float4 secondColor; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; float4 color : COLOR; };
[shader("vertex")]
Varying indexed(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID) {
    Varying result;
    float2 p = root.vertices[vertex];
    p.x = p.x * root.scale + (instance - root.firstInstance) * 2.0 * root.scale - (1.0 - root.scale);
    result.position = float4(p, 0.5, 1.0);
    result.color = instance == root.firstInstance ? root.firstColor : root.secondColor;
    return result;
}
[shader("vertex")]
Varying procedural(uint vertex : SV_VulkanVertexID) {
    float2 positions[3] = { float2(-1,-1), float2(3,-1), float2(-1,3) };
    Varying result; result.position = float4(positions[vertex], 0.5, 1.0); result.color = root.firstColor; return result;
}
[shader("fragment")]
float4 fragmentMain(Varying input) : SV_Target { return input.color; }
