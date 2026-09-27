static const float GAMMA = 2.2f;
static const float ONE_OVER_GAMMA = 0.45454545455f;

// https://knarkowicz.wordpress.com/2016/01/06/aces-filmic-tone-mapping-curve/
func aces_film(float3 x) -> float3 {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

func gamma(float3 value) -> float3 {
	return pow(value, float3(ONE_OVER_GAMMA));
}

func linear_to_srgb(float3 value) -> float3 {
    return lerp(12.92 * value, 1.055 * pow(value, float3(1.0 / 2.4)) - 0.055, step(float3(0.0031308), value));
}
