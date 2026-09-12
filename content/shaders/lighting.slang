import view;
import shading_data;
import brdf;
import screen_quad;
import sh;
import debug;
import math_constants;
import tonemapping;

struct Root {
    ViewData* view;
    float4* sh;
    uint base_color;
    uint normal_mra;
    uint depth;
    uint skybox;
    uint linear_sampler;
    uint nearest_sampler;
    uint debug_mode;
    uint padding;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

struct PSOutput {
    float4 output : SV_Target;
};

[shader("fragment")]
PSOutput main(PSInput input) {
    let ndc_pos = float4((input.uv - 0.5) * float2(2.0, -2.0), 1.0, 1.0);
    var world_pos = mul(root.view->inv_view_proj, ndc_pos);
    world_pos.xyz /= world_pos.w;

    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.linear_sampler)];
    Texture2D<float4> base_color = ResourceDescriptorHeap[NonUniformResourceIndex(root.base_color)];
    Texture2D<float4> normal_mra = ResourceDescriptorHeap[NonUniformResourceIndex(root.normal_mra)];
    let base = base_color.Sample(sampler, input.uv);
    let nmr = normal_mra.Sample(sampler, input.uv);

    float3 n;
    float metallic;
    float roughness;
    decode_normal_mra(nmr, n, metallic, roughness);

    let view_dir = normalize(world_pos.xyz - root.view->position.xyz);

    ThreeBandSH sh3[3] = { ThreeBandSH(), ThreeBandSH(), ThreeBandSH() };
    sh3[0].coeff0 = root.sh[0];
    sh3[0].coeff1 = root.sh[1];
    sh3[0].coeff2 = root.sh[2].x;

    sh3[1].coeff0 = root.sh[3];
    sh3[1].coeff1 = root.sh[4];
    sh3[1].coeff2 = root.sh[5].x;

    sh3[2].coeff0 = root.sh[6];
    sh3[2].coeff1 = root.sh[7];
    sh3[2].coeff2 = root.sh[8].x;

    Texture2D<float> depth_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
    SamplerState nearest = SamplerDescriptorHeap[NonUniformResourceIndex(root.nearest_sampler)];
    let depth = depth_image.Sample(nearest, input.uv).r;
    if (depth == 0.0) {
        TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
        let skybox_color = skybox.Sample(sampler, view_dir).rgb;

        PSOutput output;
        if ((root.debug_mode & DebugMode.DiffuseSH) != 0) {
            output.output = float4(float3(sh3[0].evaluate(view_dir), sh3[1].evaluate(view_dir), sh3[2].evaluate(view_dir)), 1.0);
        } else {
            output.output = float4(skybox_color, 1.0);
        }
        return output;
    }

    let l = float3(0.0, 0.0, 1.0);
    let light_color = float3(1.0, 1.0, 1.0);
    let v = -view_dir;

    metallic = 0.0;

    let specular_color = lerp(float3(0.04), base.xyz, metallic);
    let diffuse_color = max(1.0 - metallic, 0.0) * base.xyz;

    let alpha = roughness * roughness;
    let h = normalize(l + v);
    let f = fresnel_schlick_appro(specular_color, l, h);
    let d = ndf_ggx(n, h, alpha * alpha);
    let vis = vis_smith_joint(v, l, n, alpha);
    let specular = f * d * vis;

    let diffuse = diffuse_lambert(diffuse_color);
    let ibl_diffuse_lighting = float3(sh3[0].evaluate(n), sh3[1].evaluate(n), sh3[2].evaluate(n));
    let irradiance = diffuse * ibl_diffuse_lighting;
    let radiance = (diffuse + specular) * light_color * max(dot(n, l), 0.0);

    let final_output = gamma(aces_film(irradiance));

    PSOutput output;
    output.output = float4(final_output, 1.0);
    return output;
}
