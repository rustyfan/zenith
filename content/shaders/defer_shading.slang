import images;
import view;
import shading_data;

struct Vertex { float3 position; float3 normal; float2 tex_coord; };
struct Root {
    ViewData* view;
    Vertex* vertices;
    float4x4 model;
    float4 base_color_factor;
    uint base_color_tex;
    uint mra_tex;
    uint normal_tex;
    uint sampler;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; float2 uv : TEXCOORD0; float3 normal : NORMAL; };
[shader("vertex")]
Varying vsmain(uint id : SV_VulkanVertexID) {
    let input = root.vertices[id];
    Varying output;
    output.position = mul(root.view->view_proj, mul(root.model, float4(input.position, 1.0)));
    output.uv = input.tex_coord;
    output.normal = input.normal;
    return output;
}
struct Output { float4 base_color : SV_Target0; float4 nmr : SV_Target1; };
[shader("fragment")]
Output psmain(Varying input) {
    var base = root.base_color_factor;
    if (root.base_color_tex != 0xffffffff) { base *= sample_image(root.base_color_tex, root.sampler, input.uv); }
    var normal_ws = normalize(input.normal);
    if (root.normal_tex != 0xffffffff) { normal_ws = decode_normal_rg(sample_image(root.normal_tex, root.sampler, input.uv).xy); }
    var metallic = 0.0;
    var roughness = 1.0;
    if (root.mra_tex != 0xffffffff) {
        float3 mra = sample_image(root.mra_tex, root.sampler, input.uv).rgb;
        metallic = mra.b;
        roughness = mra.g;
    }
    Output output;
    let data = ShadingData::new(base.xyz, normal_ws, metallic, roughness);
    data.export_to_gbuffer(output.base_color, output.nmr);
    return output;
}
