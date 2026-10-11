struct Root {
    uint64_t scene;
    uint* results;
    uint offset;
    uint call_enabled;
    uint addend;
};
#include "zenith_root.slang"

struct Payload { uint value; };

[shader("raygeneration")]
void raygen() {
    uint3 id = DispatchRaysIndex();
    uint3 extent = DispatchRaysDimensions();
    uint index = id.x + extent.x * (id.y + extent.y * id.z);
    uint record = (index / 2) % 2;
    RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(root.scene);
    RayDesc ray;
    ray.Origin = float3(index % 2 == 0 ? 0.0 : 4.0, 0.0, 0.0);
    ray.Direction = float3(0.0, 1.0, 0.0);
    ray.TMin = 0.001;
    ray.TMax = 100.0;
    Payload payload;
    payload.value = 999;
    TraceRay(scene, RAY_FLAG_FORCE_OPAQUE, 0xff, record, 0, record, ray, payload);
    if (root.call_enabled != 0) {
        CallShader(0, payload);
    }
    root.results[root.offset + index] = payload.value;
}

[shader("raygeneration")]
void raygen_empty() {
    root.results[root.offset + DispatchRaysIndex().x] = root.addend + DispatchRaysIndex().x;
}

[shader("miss")]
void miss_zero(inout Payload payload) {
    payload.value = 100 + root.addend;
}

[shader("miss")]
void miss_one(inout Payload payload) {
    payload.value = 200 + root.addend;
}

[shader("closesthit")]
void hit_zero(inout Payload payload, BuiltInTriangleIntersectionAttributes attributes) {
    payload.value = 10 + root.addend;
}

[shader("closesthit")]
void hit_one(inout Payload payload, BuiltInTriangleIntersectionAttributes attributes) {
    payload.value = 20 + root.addend;
}

[shader("callable")]
void callable(inout Payload data) {
    data.value += 7;
}
