import raytracing.scene;

struct ValidationRay { float3 origin; float3 direction; };
struct Root {
    uint64_t scene;
    RayTracingInstance* instances;
    ValidationRay* rays;
    float4* output;
};
#include "zenith_root.slang"

struct Payload {
    PathTracingSurface surface;
    uint hit;
    uint instance_index;
    uint primitive_index;
    float distance;
};

[shader("raygeneration")]
void raygen() {
    let index = DispatchRaysIndex().x;
    let input = root.rays[index];
    RayDesc ray;
    ray.Origin = input.origin;
    ray.Direction = input.direction;
    ray.TMin = 0.001;
    ray.TMax = 100.0;
    Payload payload;
    payload.hit = 0;
    if (root.scene != 0) {
        RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(root.scene);
        TraceRay(scene, RAY_FLAG_FORCE_OPAQUE, 0xff, 0, 0, 0, ray, payload);
    }
    if (payload.hit == 0) {
        for (uint i = 0; i < 5; ++i) { root.output[index * 5 + i] = 0.0; }
        return;
    }
    let surface = payload.surface;
    root.output[index * 5 + 0] = float4(surface.position, 1.0);
    root.output[index * 5 + 1] = float4(surface.normal, surface.metallic);
    root.output[index * 5 + 2] = float4(surface.geometric_normal, surface.roughness);
    root.output[index * 5 + 3] = float4(surface.base_color, surface.uv.x);
    root.output[index * 5 + 4] = float4(surface.uv.y, float(payload.instance_index), float(payload.primitive_index), payload.distance);
}

[shader("miss")]
void miss(inout Payload payload) {
    payload.hit = 0;
}

[shader("closesthit")]
void closest_hit(inout Payload payload, BuiltInTriangleIntersectionAttributes attributes) {
    payload.hit = 1;
    payload.instance_index = InstanceID();
    payload.primitive_index = PrimitiveIndex();
    payload.distance = RayTCurrent();
    payload.surface = load_opaque_surface(root.instances, payload.instance_index, payload.primitive_index, attributes.barycentrics, WorldRayDirection());
}
