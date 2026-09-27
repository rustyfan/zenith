import common.math_constants;

func hair_transmittance(uint64_t acceleration, float4* materials, float3 position,
    float3 direction, float bias, float distance) -> float3 {
    if (acceleration == 0) { return 1.0; }
    RayDesc ray;
    ray.Origin = position;
    ray.Direction = direction;
    ray.TMin = bias;
    ray.TMax = distance;
    RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(acceleration);
    RayQuery<RAY_FLAG_FORCE_NON_OPAQUE> query;
    query.TraceRayInline(scene, 0, 0xff, ray);
    var transmission = float3(1.0);
    while (query.Proceed()) {
        if (query.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE) {
            let material = materials[query.CandidateInstanceID()];
            // Mean unit-radius chord approximation for unresolved shadow fibers.
            transmission *= lerp(float3(1.0), 0.92 * exp(-material.xyz * (PI * 0.5)), material.w);
            if (max(transmission.x, max(transmission.y, transmission.z)) < 0.001) {
                query.Abort();
            }
        }
    }
    return transmission;
}
