// PCG RXS-M-XS permutation and LCG constants (Jarzynski and Olano, 2020).
func path_hash(uint value) -> uint {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

func path_random(inout uint state) -> float {
    state = state * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return float(((word >> 22u) ^ word) >> 8u) / 16777216.0;
}

func path_random3(inout uint state) -> float3 {
    float3 result;
    result.x = path_random(state);
    result.y = path_random(state);
    result.z = path_random(state);
    return result;
}

func offset_path_origin(float3 position, float3 normal, float minimum_bias) -> float3 {
    let scale = max(max(abs(position.x), abs(position.y)), abs(position.z));
    // Scale the offset by float32 precision away from the world origin.
    return position + normal * max(minimum_bias, scale * (8.0 / 8388608.0));
}
