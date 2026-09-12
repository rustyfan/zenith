struct PSInput { float2 uv : TEXCOORD0; };
struct VSOutput { float4 position : SV_Position; PSInput psout; };
[shader("vertex")]
VSOutput vsmain(uint id : SV_VulkanVertexID) {
    let uv = float2((id << 1) & 2, id & 2);
    VSOutput output;
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    output.psout.uv = uv;
    return output;
}
