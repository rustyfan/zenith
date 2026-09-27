import common.images;
import common.view;
import shading.shading_data;

struct Vertex { float3 position; float3 normal; float2 tex_coord; float4 tangent; };
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
    float tangent_sign;
    uint padding;
    float coat_weight;
    float coat_roughness;
    float coat_normal_scale;
    uint coat_texture;
    uint coat_roughness_texture;
    uint coat_normal_texture;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
struct Varying { float4 position : SV_Position; float2 uv : TEXCOORD0; float3 normal : NORMAL; float4 tangent : TANGENT; };
[shader("vertex")]
Varying vsmain(uint id : SV_VulkanVertexID) {
    let input = root.vertices[id];
    Varying output;
    let world = mul(root.model, float4(input.position, 1.0));
    output.position = mul(root.view->view_proj, world);
    output.uv = input.tex_coord;
    output.normal = normalize(mul(root.normal_matrix, float4(input.normal, 0.0)).xyz);
    let tangent_ws = mul(root.model, float4(input.tangent.xyz, 0.0)).xyz;
    output.tangent = float4(input.tangent.w != 0.0 ? normalize(tangent_ws) : float3(0.0), input.tangent.w * root.tangent_sign);
    return output;
}
struct Output { float4 base_color : SV_Target0; float4 nmr : SV_Target1; float4 coat : SV_Target2; };
[shader("fragment")]
Output psmain(Varying input) {
    var base = root.base_color_factor;
    if (root.base_color_tex != 0xffffffff) { base *= sample_image(root.base_color_tex, root.sampler, input.uv); }
    var normal_ws = normalize(input.normal);
    var coat_normal = normal_ws;
    if (root.coat_normal_texture != 0xffffffff && input.tangent.w != 0.0) {
        var ts = decode_normal_rg(sample_image(root.coat_normal_texture, root.sampler, input.uv).xy);
        ts.xy *= root.coat_normal_scale;
        let t = normalize(input.tangent.xyz - normal_ws * dot(normal_ws, input.tangent.xyz));
        let b = cross(normal_ws, t) * input.tangent.w;
        coat_normal = normalize(t * ts.x + b * ts.y + normal_ws * ts.z);
    }
    if (root.normal_tex != 0xffffffff) {
        let normal_ts = decode_normal_rg(sample_image(root.normal_tex, root.sampler, input.uv).xy);
        if (input.tangent.w != 0.0) {
            let t = normalize(input.tangent.xyz - normal_ws * dot(normal_ws, input.tangent.xyz));
            let b = cross(normal_ws, t) * input.tangent.w;
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
    var weight = root.coat_weight;
    var coat_roughness = root.coat_roughness;
    if (root.coat_texture != 0xffffffff) { weight *= sample_image(root.coat_texture, root.sampler, input.uv).r; }
    if (root.coat_roughness_texture != 0xffffffff) { coat_roughness *= sample_image(root.coat_roughness_texture, root.sampler, input.uv).g; }
    output.coat = float4(encode_normal_octahedral(coat_normal), saturate(weight), saturate(coat_roughness));
    return output;
}
