struct Root { uint64_t scene; uint* results; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
[shader("compute")]
[numthreads(1, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(root.scene);
    RayDesc ray;
    ray.Origin = float3(0.0, id.x == 0 ? 0.0 : (id.x == 1 ? 1.999 : 3.0), 0.0);
    ray.Direction = float3(0.0, id.x == 0 ? 1.0 : -1.0, 0.0);
    ray.TMin = 0.001;
    ray.TMax = 100.0;
    RayQuery<RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH> query;
    query.TraceRayInline(scene, 0, 0xff, ray);
    while (query.Proceed()) {}
    root.results[id.x] = query.CommittedStatus();
}
