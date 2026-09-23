import view;
import shading_data;
import ibl_sampling;
import math_constants;
import ray_query;

struct Root {
    ViewData* view;
    uint64_t acceleration;
    uint normal_mra;
    uint depth;
    uint samples;
    float radius;
    float bias;
    uint padding;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

func ambient_visibility(float3 position, float3 normal, int2 pixel) -> float {
    let axis = abs(normal.z) < 0.999 ? float3(0.0, 0.0, 1.0) : float3(1.0, 0.0, 0.0);
    let tangent = normalize(cross(axis, normal));
    let bitangent = cross(normal, tangent);
    // Interleaved gradient noise rotates the fixed sample set per pixel.
    let rotation = frac(52.9829189 * frac(dot(float2(pixel), float2(0.06711056, 0.00583715))));
    float visibility = 0.0;
    for (uint i = 0; i < root.samples; ++i) {
        let u = hammersley(i, root.samples);
        let radius_squared = u.x + 0.5 / float(root.samples);
        let phi = 2.0 * PI * frac(u.y + rotation);
        let radial = sqrt(radius_squared);
        let direction = tangent * (radial * cos(phi)) + bitangent * (radial * sin(phi))
            + normal * sqrt(1.0 - radius_squared);
        visibility += ray_visibility(root.acceleration, position, normal, direction, root.bias, root.radius);
    }
    return visibility / float(root.samples);
}

[shader("fragment")]
float main(float4 raster_position : SV_Position) : SV_Target {
    Texture2D<float> depth_image = ResourceDescriptorHeap[NonUniformResourceIndex(root.depth)];
    uint width, height;
    depth_image.GetDimensions(width, height);
    let pixel = int3(int2(raster_position.xy) * 2, 0);
    let uv = (float2(pixel.xy) + 0.5) / float2(width, height);
    let depth = depth_image.Load(pixel);
    if (depth == 0.0) {
        return 1.0;
    }
    Texture2D<float4> normal_mra = ResourceDescriptorHeap[NonUniformResourceIndex(root.normal_mra)];
    let shading_normal = decode_normal_octahedral(normal_mra.Load(pixel).xy);
    let position = surface_position(root.view, uv, depth);
    let ndc_pos = float4((uv - 0.5) * float2(2.0, -2.0), 1.0, 1.0);
    let world_pos = mul(root.view->inv_view_proj, ndc_pos);
    let view_direction = -normalize(world_pos.xyz / world_pos.w - root.view->position.xyz);
    let normal = surface_normal(root.view, depth_image, pixel.xy, int2(width, height),
        depth, position, view_direction, shading_normal);
    return ambient_visibility(position, normal, int2(raster_position.xy));
}
