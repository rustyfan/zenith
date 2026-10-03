import shading.shading_model;

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

struct ShadingData {
    float3 basecolor;
    float3 normal;
    float metallic;
    float roughness;
    float ao;
    ShadingModel shading_model;

    static func new(float3 basecolor, float3 normal, float metallic, float roughness, float ao, ShadingModel shading_model) -> ShadingData {
        ShadingData data;
        data.basecolor = basecolor;
        data.normal = normal;
        data.metallic = metallic;
        data.roughness = roughness;
        data.ao = ao;
        data.shading_model = shading_model;
        return data;
    }

    static func from_gbuffer(float2 a, float4 b, float4 c) -> ShadingData {
        return ShadingData.new(b.rgb, decode_normal_octahedral(a), c.r, c.g, c.b, ShadingModel(uint(round(c.a * 255.0))));
    }

    func export_to_gbuffer(out float2 a, out float4 b, out float4 c) {
        a = encode_normal_octahedral(normalize(normal));
        b = float4(basecolor, 1.0);
        c = float4(saturate(metallic), saturate(roughness), saturate(ao), float(uint(shading_model)) / 255.0);
    }
};

struct CoatData {
    float3 normal;
    float weight;
    float roughness;
    static func from_gbuffer(float4 packed) -> CoatData {
        CoatData data;
        data.normal = decode_normal_octahedral(packed.xy);
        data.weight = packed.z;
        data.roughness = packed.w;
        return data;
    }
    static func to_gbuffer(float3 normal, float weight, float roughness) -> float4 {
        return float4(encode_normal_octahedral(normal), saturate(weight), saturate(roughness));
    }
};
