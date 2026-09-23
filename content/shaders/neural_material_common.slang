static const float MATERIAL_TAU = 6.283185307179586;

float feature(float2 uv, uint index) {
    float2 p = uv * MATERIAL_TAU;
    float phases[8] = {p.x, p.y, p.x + p.y, p.x - p.y, 2.0 * p.x,
        2.0 * p.y, 2.0 * p.x + p.y, p.x + 2.0 * p.y};
    return (index & 1) == 0 ? sin(phases[index / 2]) : cos(phases[index / 2]);
}

float4 reference_material(float2 uv) {
    float2 p = uv * MATERIAL_TAU;
    float wave = sin(p.x + 0.55 * sin(p.y) + 0.3 * sin(2.0 * p.y));
    float band = 0.5 + 0.5 * wave;
    float vein = max(1.0 - abs(wave - 0.15) * 6.0, 0.0);
    vein *= vein;
    float grain = 0.5 + 0.5 * cos(p.x + 2.0 * p.y);
    float3 base = float3(0.025 + 0.06 * band, 0.10 + 0.26 * band, 0.14 + 0.29 * band);
    return float4(lerp(base, float3(0.72, 0.37, 0.09), vein), 0.22 + 0.18 * grain + 0.18 * vein);
}
