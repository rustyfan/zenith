import view;

func surface_position(ViewData* view, float2 uv, float depth) -> float3 {
    let clip = float4((uv - 0.5) * float2(2.0, -2.0), depth, 1.0);
    let world = mul(view->inv_view_proj, clip);
    return world.xyz / world.w;
}

func surface_neighbor(ViewData* view, Texture2D<float> depth_image, int2 pixel, int2 offset,
    int2 extent, float depth, float3 position, out float error) -> float3 {
    let neighbor = clamp(pixel + offset, int2(0), extent - 1);
    let neighbor_depth = depth_image.Load(int3(neighbor, 0));
    error = neighbor_depth > 0.0 && any(neighbor != pixel) ? abs(neighbor_depth - depth) : 1e30;
    return error < 1e30
        ? surface_position(view, (float2(neighbor) + 0.5) / float2(extent), neighbor_depth) - position
        : float3(0.0);
}

func surface_normal(ViewData* view, Texture2D<float> depth_image, int2 pixel, int2 extent,
    float depth, float3 position, float3 view_direction, float3 shading_normal) -> float3 {
    float left_error, right_error, up_error, down_error;
    let left = surface_neighbor(view, depth_image, pixel, int2(-1, 0), extent, depth, position, left_error);
    let right = surface_neighbor(view, depth_image, pixel, int2(1, 0), extent, depth, position, right_error);
    let up = surface_neighbor(view, depth_image, pixel, int2(0, -1), extent, depth, position, up_error);
    let down = surface_neighbor(view, depth_image, pixel, int2(0, 1), extent, depth, position, down_error);
    let dx = left_error < right_error ? -left : right;
    let dy = up_error < down_error ? -up : down;
    var normal = cross(dx, dy);
    normal = dot(normal, normal) > 1e-20 ? normalize(normal) : shading_normal;
    return normal * (dot(normal, view_direction) >= 0.0 ? 1.0 : -1.0);
}

func ray_visibility(uint64_t acceleration, float3 position, float3 normal, float3 direction,
    float minimum_bias, float distance) -> float {
    let scale = max(max(abs(position.x), abs(position.y)), abs(position.z));
    // Scale the bias by float32 precision away from the world origin.
    let bias = max(minimum_bias, scale * (8.0 / 8388608.0));
    RayDesc ray;
    ray.Origin = position + normal * bias;
    ray.Direction = direction;
    ray.TMin = minimum_bias;
    ray.TMax = distance;
    RaytracingAccelerationStructure scene = RaytracingAccelerationStructure(acceleration);
    RayQuery<RAY_FLAG_FORCE_OPAQUE | RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH> query;
    query.TraceRayInline(scene, 0, 0xff, ray);
    while (query.Proceed()) {}
    return query.CommittedStatus() == COMMITTED_TRIANGLE_HIT ? 0.0 : 1.0;
}
