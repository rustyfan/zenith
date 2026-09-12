import images;
import view;
import shading_data;

struct Vertex { float3 position; float3 normal; float2 tex_coord; };
struct Root {
    ViewData* view;
    Vertex* vertices;
    float4x4 model;
    float4x4 normal_matrix;
    float4 base_color_factor;
    uint base_color_tex;
    uint mra_tex;
    uint normal_tex;
    uint sampler;
    float metallic;
    float roughness;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; float2 uv : TEXCOORD0; float3 normal : NORMAL; float3 world_position : TEXCOORD1; };
[shader("vertex")]
Varying vsmain(uint id : SV_VulkanVertexID) {
    let input = root.vertices[id];
    Varying output;
    let world = mul(root.model, float4(input.position, 1.0));
    output.position = mul(root.view->view_proj, world);
    output.world_position = world.xyz;
    output.uv = input.tex_coord;
    output.normal = mul(root.normal_matrix, float4(input.normal, 0.0)).xyz;
    return output;
}
struct Output { float4 base_color : SV_Target0; float4 nmr : SV_Target1; };
[shader("fragment")]
Output psmain(Varying input) {
    var base = root.base_color_factor;
    if (root.base_color_tex != 0xffffffff) { base *= sample_image(root.base_color_tex, root.sampler, input.uv); }
    var normal_ws = normalize(input.normal);
    if (root.normal_tex != 0xffffffff) {
        let dp1 = ddx(input.world_position);
        let dp2 = ddy(input.world_position);
        let duv1 = ddx(input.uv);
        let duv2 = ddy(input.uv);
        let determinant = duv1.x * duv2.y - duv1.y * duv2.x;
        if (abs(determinant) > 1e-8) {
            let tangent = (dp1 * duv2.y - dp2 * duv1.y) / determinant;
            let bitangent = (dp2 * duv1.x - dp1 * duv2.x) / determinant;
            let t = normalize(tangent - normal_ws * dot(normal_ws, tangent));
            let b = normalize(cross(normal_ws, t)) * (dot(cross(normal_ws, t), bitangent) < 0.0 ? -1.0 : 1.0);
            let normal_ts = decode_normal_rg(sample_image(root.normal_tex, root.sampler, input.uv).xy);
            normal_ws = normalize(t * normal_ts.x + b * normal_ts.y + normal_ws * normal_ts.z);
        }
    }
    var metallic = root.metallic;
    var roughness = root.roughness;
    if (root.mra_tex != 0xffffffff) {
        float3 mra = sample_image(root.mra_tex, root.sampler, input.uv).rgb;
        metallic *= mra.b;
        roughness *= mra.g;
    }
    Output output;
    let data = ShadingData::new(base.xyz, normal_ws, metallic, roughness);
    data.export_to_gbuffer(output.base_color, output.nmr);
    return output;
}
