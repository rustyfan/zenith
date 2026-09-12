func oct_sign(float2 value) -> float2 {
    return float2(value.x >= 0.0 ? 1.0 : -1.0, value.y >= 0.0 ? 1.0 : -1.0);
}

func encode_normal_octahedral(float3 normal) -> float2 {
    normal /= abs(normal.x) + abs(normal.y) + abs(normal.z);
    let xy = normal.z >= 0.0 ? normal.xy : (1.0 - abs(normal.yx)) * oct_sign(normal.xy);
    return xy * 0.5 + 0.5;
}

func decode_normal_octahedral(float2 packed) -> float3 {
    let xy = packed * 2.0 - 1.0;
    var normal = float3(xy, 1.0 - abs(xy.x) - abs(xy.y));
    let t = saturate(-normal.z);
    normal.xy -= oct_sign(normal.xy) * t;
    return normalize(normal);
}

func encode_normal_rg(float3 normal) -> float2 {
    return normal.xy * 0.5 + 0.5;
}

func decode_normal_rg(float2 packed) -> float3 {
    float2 xy = packed * 2.0 - 1.0;
    float z = sqrt(saturate(1.0 - dot(xy, xy)));
    return normalize(float3(xy, z));
}

func encode_normal_mra(float3 normal, float metallic, float roughness) -> float4 {
    float2 packed_normal = encode_normal_octahedral(normalize(normal));
    return float4(packed_normal.x, packed_normal.y, metallic, roughness);
}

func decode_normal_mra(float4 packed, out float3 normal, out float metallic, out float roughness) {
    normal = decode_normal_octahedral(packed.xy);
    metallic = packed.z;
    roughness = packed.w;
}

struct ShadingData {
    float3 basecolor;
    float3 normal;
    float metallic;
    float roughness;

    static func new(float3 basecolor, float3 normal, float metallic, float roughness) -> ShadingData {
        ShadingData data;
        data.basecolor = basecolor;
        data.normal = normal;
        data.metallic = metallic;
        data.roughness = roughness;
        return data;
    }

    func export_to_gbuffer(out float4 gbuffer_basecolor, out float4 gbuffer_normal_mra) {
        gbuffer_basecolor = float4(basecolor, 1.0);
        gbuffer_normal_mra = encode_normal_mra(normal, metallic, roughness);
    }
};
