import zenith_bindings;

struct Root { uint* output; uint value; };
#include "zenith_root.slang"

struct Payload { uint value; };

[shader("raygeneration")]
void raygen() {
    root.output[0] = root.value;
}

[shader("miss")]
void miss(inout Payload payload) {
    payload.value = root.value;
}

[shader("closesthit")]
void closest_hit(inout Payload payload, BuiltInTriangleIntersectionAttributes attributes) {
    payload.value = root.value + uint(attributes.barycentrics.x);
}

[shader("anyhit")]
void any_hit(inout Payload payload, BuiltInTriangleIntersectionAttributes attributes) {
    if (payload.value == root.value && attributes.barycentrics.x > 0.0) { IgnoreHit(); }
}

[shader("intersection")]
void intersection() {
    BuiltInTriangleIntersectionAttributes attributes;
    attributes.barycentrics = float2(0.25, 0.5);
    ReportHit(float(root.value), 0, attributes);
}

[shader("callable")]
void callable(inout Payload data) {
    data.value = root.value;
}
