import zenith_bindings;
import common.view;
import brdf.brdf;
import brdf.ibl_sampling;
import brdf.opaque;
import raytracing.scene;
import raytracing.sampling;

struct Root {
    ViewData* view;
    RayTracingInstance* instances;
    uint64_t acceleration;
    tex output;
    tex skybox;
    tex brdf_lut;
    tex brdf_average;
    sampler sampler;
    uint first_sample;
    uint sample_count;
    uint max_bounces;
    float4 direction_to_light;
    float4 light_radiance;
    float sky_intensity;
    float ray_bias;
    uint multiple_scattering;
    uint accumulate;
};
#include "zenith_root.slang"

struct Payload {
    uint instance_index;
    uint primitive_index;
    float2 barycentrics;
};

func trace_path_ray(RayDesc ray, uint flags) -> Payload {
    Payload payload;
    payload.instance_index = 0xffffffff;
    if (root.acceleration != 0) {
        RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(root.acceleration);
        TraceRay(scene, flags | RAY_FLAG_FORCE_OPAQUE, 0xff, 0, 0, 0, ray, payload);
    }
    return payload;
}

func path_visibility(RayDesc ray) -> bool {
    Payload payload;
    payload.instance_index = 0;
    RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(root.acceleration);
    TraceRay(scene, RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH | RAY_FLAG_SKIP_CLOSEST_HIT_SHADER, 0xff, 0, 0, 0, ray, payload);
    return payload.instance_index == 0xffffffff;
}

func path_integral(float cosine, float roughness) -> float2 {
    Texture2D<float2> lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.brdf_lut)];
    SamplerState state = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
    return lut.SampleLevel(state, brdf_lut_uv(cosine, roughness), 0.0);
}

func path_average(float roughness) -> float2 {
    Texture2D<float2> lut = ResourceDescriptorHeap[NonUniformResourceIndex(root.brdf_average)];
    SamplerState state = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
    return lut.SampleLevel(state, float2(0.5, roughness), 0.0);
}

func path_environment(float3 direction) -> float3 {
    TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
    SamplerState state = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
    return skybox.SampleLevel(state, direction, 0.0).rgb * root.sky_intensity;
}

func integrate_path(RayDesc ray, inout uint state) -> float3 {
    float3 radiance = 0.0;
    float3 throughput = 1.0;
    for (uint bounce = 0; bounce <= root.max_bounces; ++bounce) {
        let payload = trace_path_ray(ray, 0);
        if (payload.instance_index == 0xffffffff) {
            radiance += throughput * path_environment(ray.Direction);
            break;
        }
        if (bounce == root.max_bounces) { break; }
        let surface = load_opaque_surface(root.instances, payload.instance_index, payload.primitive_index, payload.barycentrics, ray.Direction);
        let v = -ray.Direction;
        let nov = dot(surface.normal, v);
        if (nov <= 0.0) { break; }
        let roughness = clamp(surface.roughness, MIN_ROUGHNESS, 1.0);
        let f0 = lerp(float3(0.04), surface.base_color, surface.metallic);
        var bxdf = OpaqueBxdf.new(surface.normal, v, roughness, f0, (1.0 - surface.metallic) * surface.base_color,
            path_integral(nov, roughness), path_average(roughness));
        if (root.multiple_scattering == 0) { bxdf.energy.multiple_scattering = 0.0; }
        let l = root.direction_to_light.xyz;
        let nol = dot(surface.normal, l);
        ray.Origin = offset_path_origin(surface.position, surface.geometric_normal, root.ray_bias);
        ray.TMin = 0.0;
        if (nol > 0.0 && dot(surface.geometric_normal, l) > 0.0 && any(root.light_radiance.xyz > 0.0)) {
            ray.Direction = l;
            if (path_visibility(ray)) {
                radiance += throughput * bxdf.eval(l, path_integral(nol, roughness)) * root.light_radiance.xyz * nol;
            }
        }
        let sampled = bxdf.sample(path_random3(state));
        if (sampled.pdf <= 0.0 || dot(surface.geometric_normal, sampled.direction) <= 0.0) { break; }
        let cosine = dot(surface.normal, sampled.direction);
        throughput *= bxdf.eval(sampled.direction, path_integral(cosine, roughness)) * cosine / sampled.pdf;
        if (!all(isfinite(throughput)) || !any(throughput > 0.0)) { break; }
        ray.Direction = sampled.direction;
    }
    return radiance;
}

[shader("raygeneration")]
void raygen() {
    let pixel = DispatchRaysIndex().xy;
    let extent = DispatchRaysDimensions().xy;
    let pixel_index = pixel.x + pixel.y * extent.x;
    float3 sum = 0.0;
    for (uint sample = 0; sample < root.sample_count; ++sample) {
        uint state = path_hash(pixel_index ^ path_hash(root.first_sample + sample));
        let jitter = path_random3(state).xy;
        let uv = (float2(pixel) + jitter) / float2(extent);
        let clip = float4((uv - 0.5) * float2(2.0, -2.0), 1.0, 1.0);
        let world = mul(root.view->inv_view_proj, clip);
        RayDesc ray;
        ray.Origin = root.view->position.xyz;
        ray.Direction = normalize(world.xyz / world.w - ray.Origin);
        ray.TMin = 0.0;
        ray.TMax = 1e30;
        sum += integrate_path(ray, state);
    }
    RWTexture2D<float4> output = ResourceDescriptorHeap[NonUniformResourceIndex(root.output)];
    let batch_mean = sum / float(root.sample_count);
    float3 mean = batch_mean;
    if (root.accumulate != 0 && root.first_sample > 0) {
        let weight = float(root.sample_count) / float(root.first_sample + root.sample_count);
        mean = lerp(output[pixel].rgb, batch_mean, weight);
    }
    output[pixel] = float4(mean, 1.0);
}

[shader("miss")]
void miss(inout Payload payload) {
    payload.instance_index = 0xffffffff;
}

[shader("closesthit")]
void closest_hit(inout Payload payload, BuiltInTriangleIntersectionAttributes attributes) {
    payload.instance_index = InstanceID();
    payload.primitive_index = PrimitiveIndex();
    payload.barycentrics = attributes.barycentrics;
}
