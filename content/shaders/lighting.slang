import view;
import shading_data;
import ibl_sampling;
import brdf;
import screen_quad;
import sh;
import debug;

struct Root {
    ViewData* view;
    float4* sh;
    uint base_color;
    uint normal_mra;
    uint depth;
    uint skybox;
    uint specular;
    uint brdf_lut;
    uint linear_sampler;
    uint debug_mode;
    float4 direction_to_light;
    float4 light_radiance;
    float sky_intensity;
    float max_mip;
    uint64_t acceleration;
    float shadow_bias;
    float shadow_distance;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

struct PSOutput { float4 output : SV_Target; };

func surface_position(float2 uv, float depth) -> float3 {
    let clip = float4((uv - 0.5) * float2(2.0, -2.0), depth, 1.0);
    let world = mul(root.view->inv_view_proj, clip);
    return world.xyz / world.w;
}

func surface_neighbor(Texture2D<float> depth_image, int2 pixel, int2 offset,
    int2 extent, float depth, float3 position, out float error) -> float3 {
    let neighbor = clamp(pixel + offset, int2(0), extent - 1);
    let neighbor_depth = depth_image.Load(int3(neighbor, 0));
    error = neighbor_depth > 0.0 && any(neighbor != pixel) ? abs(neighbor_depth - depth) : 1e30;
    return error < 1e30
        ? surface_position((float2(neighbor) + 0.5) / float2(extent), neighbor_depth) - position
        : float3(0.0);
}

func shadow_visibility(Texture2D<float> depth_image, int2 pixel, int2 extent,
    float depth, float3 position, float3 view_direction, float3 shading_normal) -> float {
    float left_error, right_error, up_error, down_error;
    let left = surface_neighbor(depth_image, pixel, int2(-1, 0), extent, depth, position, left_error);
    let right = surface_neighbor(depth_image, pixel, int2(1, 0), extent, depth, position, right_error);
    let up = surface_neighbor(depth_image, pixel, int2(0, -1), extent, depth, position, up_error);
    let down = surface_neighbor(depth_image, pixel, int2(0, 1), extent, depth, position, down_error);
    let dx = left_error < right_error ? -left : right;
    let dy = up_error < down_error ? -up : down;
    var normal = cross(dx, dy);
    normal = dot(normal, normal) > 1e-20 ? normalize(normal) : shading_normal;
    normal *= dot(normal, view_direction) >= 0.0 ? 1.0 : -1.0;
    let scale = max(max(abs(position.x), abs(position.y)), abs(position.z));
    // Scale the bias by float32 precision away from the world origin.
    let bias = max(root.shadow_bias, scale * (8.0 / 8388608.0));
    RayDesc ray;
    ray.Origin = position + normal * bias;
    ray.Direction = root.direction_to_light.xyz;
    ray.TMin = root.shadow_bias;
    ray.TMax = root.shadow_distance;
    RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(root.acceleration);
    RayQuery<RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH> query;
    query.TraceRayInline(scene, 0, 0xff, ray);
    while (query.Proceed()) {}
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT ? 0.0 : 1.0;
}

[shader("fragment")]
PSOutput main(PSInput input) {
    let ndc_pos = float4((input.uv - 0.5) * float2(2.0, -2.0), 1.0, 1.0);
    let world_pos = mul(root.view->inv_view_proj, ndc_pos);
    let view_dir = normalize(world_pos.xyz / world_pos.w - root.view->position.xyz);
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.linear_sampler)];
    Texture2D<float> depth_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
    uint width, height;
    depth_image.GetDimensions(width, height);
    let pixel = int3(int2(input.uv * float2(width, height)), 0);
    let depth = depth_image.Load(pixel);

    PSOutput output;
    if (depth == 0.0) {
        TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
        let radiance = (root.debug_mode & DebugMode.DiffuseSH) != 0
            ? evaluate_diffuse_sh(root.sh, view_dir)
            : skybox.SampleLevel(sampler, view_dir, 0).rgb;
        output.output = float4(radiance * root.sky_intensity, 1.0);
        return output;
    }

    Texture2D<float4> base_color = ResourceDescriptorHeap[NonUniformResourceIndex(root.base_color)];
    Texture2D<float4> normal_mra = ResourceDescriptorHeap[NonUniformResourceIndex(root.normal_mra)];
    let base = base_color.Load(pixel).rgb;
    float3 n;
    float metallic, roughness;
    decode_normal_mra(normal_mra.Load(pixel), n, metallic, roughness);
    metallic = saturate(metallic);
    roughness = clamp(roughness, MIN_ROUGHNESS, 1.0);
    let v = -view_dir;
    let nov = saturate(dot(n, v));
    let specular_color = lerp(float3(0.04), base, metallic);
    let diffuse_color = (1.0 - metallic) * base;
    let alpha = roughness * roughness;

    float3 direct = 0.0;
    let l = root.direction_to_light.xyz;
    let nol = saturate(dot(n, l));
    if (nol > 0.0 && nov > 0.0 && dot(l + v, l + v) > 1e-8) {
        let h = normalize(l + v);
        let f = fresnel_schlick_appro(specular_color, l, h);
        let specular = f * ndf_ggx(n, h, alpha * alpha) * vis_smith_joint(v, l, n, alpha);
        direct = ((1.0 - f) * diffuse_lambert(diffuse_color) + specular) * root.light_radiance.xyz * nol;
        if (root.acceleration != 0 && any(root.light_radiance.xyz > 0.0)) {
            let position = surface_position(input.uv, depth);
            direct *= shadow_visibility(depth_image, pixel.xy, int2(width, height), depth, position, v, n);
        }
    }

    let ambient_f = fresnel_schlick_roughness_appro(specular_color, n, v, roughness);
    let diffuse_ibl = (1.0 - ambient_f) * diffuse_color * evaluate_diffuse_sh(root.sh, n);
    TextureCube<float4> environment = ResourceDescriptorHeap[NonUniformResourceIndex(root.specular)];
    Texture2D<float2> brdf_lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.brdf_lut)];
    let integrated = brdf_lut.SampleLevel(sampler, float2(nov, roughness), 0);
    let reflected = environment.SampleLevel(sampler, reflect(-v, n), roughness * root.max_mip).rgb;
    let specular_ibl = reflected * (specular_color * integrated.x + integrated.y);
    output.output = float4(direct + (diffuse_ibl + specular_ibl) * root.sky_intensity, 1.0);
    return output;
}
