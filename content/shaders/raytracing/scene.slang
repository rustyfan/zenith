import zenith_bindings;
import shading.shading_data;

struct RayTracingVertex {
    float3 position;
    float3 normal;
    float2 tex_coord;
    float4 tangent;
};

struct RayTracingInstance {
    RayTracingVertex* vertices;
    uint* indices;
    float4x4 model;
    float4x4 normal_matrix;
    float4 base_color;
    tex base_texture;
    tex mra_texture;
    tex normal_texture;
    sampler sampler;
    float metallic;
    float roughness;
    float tangent_sign;
    uint padding;
};

struct PathTracingSurface {
    float3 position;
    float3 geometric_normal;
    float3 normal;
    float2 uv;
    float3 base_color;
    float metallic;
    float roughness;
};

func sample_material_texture(tex image_index, sampler sampler_index, float2 uv) -> float4 {
    Texture2D<float4> image = ResourceDescriptorHeap[NonUniformResourceIndex(image_index)];
    SamplerState state = SamplerDescriptorHeap[NonUniformResourceIndex(sampler_index)];
    return image.SampleLevel(state, uv, 0.0);
}

func load_opaque_surface(RayTracingInstance* instances, uint instance_index, uint primitive_index, float2 barycentrics, float3 ray_direction) -> PathTracingSurface {
    let instance = instances[instance_index];
    let a = instance.vertices[instance.indices[primitive_index * 3 + 0]];
    let b = instance.vertices[instance.indices[primitive_index * 3 + 1]];
    let c = instance.vertices[instance.indices[primitive_index * 3 + 2]];
    let weights = float3(1.0 - barycentrics.x - barycentrics.y, barycentrics.x, barycentrics.y);
    let p0 = mul(instance.model, float4(a.position, 1.0)).xyz;
    let p1 = mul(instance.model, float4(b.position, 1.0)).xyz;
    let p2 = mul(instance.model, float4(c.position, 1.0)).xyz;
    PathTracingSurface surface;
    surface.position = p0 * weights.x + p1 * weights.y + p2 * weights.z;
    surface.geometric_normal = normalize(cross(p1 - p0, p2 - p0));
    if (dot(surface.geometric_normal, ray_direction) > 0.0) {
        surface.geometric_normal = -surface.geometric_normal;
    }
    surface.normal = normalize(mul(instance.normal_matrix, float4(a.normal, 0.0)).xyz) * weights.x
        + normalize(mul(instance.normal_matrix, float4(b.normal, 0.0)).xyz) * weights.y
        + normalize(mul(instance.normal_matrix, float4(c.normal, 0.0)).xyz) * weights.z;
    surface.normal = dot(surface.normal, surface.normal) > 0.0 ? normalize(surface.normal) : surface.geometric_normal;
    surface.uv = a.tex_coord * weights.x + b.tex_coord * weights.y + c.tex_coord * weights.z;
    var base = instance.base_color;
    if (instance.base_texture != 0xffffffff) {
        base *= sample_material_texture(instance.base_texture, instance.sampler, surface.uv);
    }
    surface.base_color = base.rgb;
    surface.metallic = instance.metallic;
    surface.roughness = instance.roughness;
    if (instance.mra_texture != 0xffffffff) {
        let mra = sample_material_texture(instance.mra_texture, instance.sampler, surface.uv).rgb;
        surface.metallic *= mra.b;
        surface.roughness *= mra.g;
    }
    surface.metallic = saturate(surface.metallic);
    surface.roughness = saturate(surface.roughness);
    if (instance.normal_texture != 0xffffffff && a.tangent.w != 0.0 && b.tangent.w != 0.0 && c.tangent.w != 0.0) {
        var tangent = normalize(mul(instance.model, float4(a.tangent.xyz, 0.0)).xyz) * weights.x
            + normalize(mul(instance.model, float4(b.tangent.xyz, 0.0)).xyz) * weights.y
            + normalize(mul(instance.model, float4(c.tangent.xyz, 0.0)).xyz) * weights.z;
        tangent -= surface.normal * dot(surface.normal, tangent);
        if (dot(tangent, tangent) > 0.0) {
            tangent = normalize(tangent);
            let handedness = (a.tangent.w * weights.x + b.tangent.w * weights.y + c.tangent.w * weights.z) * instance.tangent_sign;
            let bitangent = cross(surface.normal, tangent) * handedness;
            let normal_ts = decode_normal_rg(sample_material_texture(instance.normal_texture, instance.sampler, surface.uv).xy);
            surface.normal = normalize(tangent * normal_ts.x + bitangent * normal_ts.y + surface.normal * normal_ts.z);
        }
    }
    if (dot(surface.normal, surface.geometric_normal) < 0.0) {
        surface.normal = -surface.normal;
    }
    return surface;
}
