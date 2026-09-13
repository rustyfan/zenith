import math_constants;

static const float MIN_ROUGHNESS = 0.045;

func hammersley(uint i, uint count) -> float2 {
    return float2(float(i) / float(count), float(reversebits(i)) * 2.3283064365386963e-10);
}

func sample_ggx(float2 u, float alpha) -> float3 {
    let phi = 2.0 * PI * u.x;
    let cos_theta = sqrt((1.0 - u.y) / (1.0 + (alpha * alpha - 1.0) * u.y));
    let sin_theta = sqrt(saturate(1.0 - cos_theta * cos_theta));
    return float3(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
}

func cubemap_direction(uint face, float2 uv) -> float3 {
    let p = uv * 2.0 - 1.0;
    switch (face) {
        case 0: return normalize(float3(1.0, -p.y, -p.x));
        case 1: return normalize(float3(-1.0, -p.y, p.x));
        case 2: return normalize(float3(p.x, 1.0, p.y));
        case 3: return normalize(float3(p.x, -1.0, -p.y));
        case 4: return normalize(float3(p.x, -p.y, 1.0));
        default: return normalize(float3(-p.x, -p.y, -1.0));
    }
}
