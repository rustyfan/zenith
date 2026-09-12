struct Vertex { float3 position; float3 color; };
struct Root { Vertex* vertices; float time; uint padding; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; float3 color : COLOR; };
[shader("vertex")]
Varying vsmain(uint id : SV_VulkanVertexID) {
    Varying output;
    output.position = float4(root.vertices[id].position, 1.0);
    output.color = root.vertices[id].color;
    return output;
}
[shader("fragment")]
float4 psmain(Varying input) : SV_Target {
    let animated = input.color * (0.5 + 0.5 * sin(root.time * 3.0 + input.color * 6.28));
    return float4(animated, 1.0);
}
