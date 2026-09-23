import math_constants;

// Building an Orthonormal Basis, Revisited
// http://jcgt.org/published/0006/01/01/
func build_orthonormal_basis(float3 n) -> float3x3 {
    float3 b1;
    float3 b2;

    if (n.z < 0.0) {
        const float a = 1.0 / (1.0 - n.z);
        const float b = n.x * n.y * a;
        b1 = float3(1.0 - n.x * n.x * a, -b, n.x);
        b2 = float3(b, n.y * n.y * a - 1.0, -n.y);
    } else {
        const float a = 1.0 / (1.0 + n.z);
        const float b = -n.x * n.y * a;
        b1 = float3(1.0 - n.x * n.x * a, b, -n.x);
        b2 = float3(b, 1.0 - n.y * n.y * a, -n.y);
    }

    return float3x3(
        b1.x, b2.x, n.x,
        b1.y, b2.y, n.y,
        b1.z, b2.z, n.z
    );
}

// Fresnel phenomenon describe the ratio between reflection and refraction based on the light vector and medium IOR.
// Schlick approximation to Fresnel phenomenon for microfacet model.
// Replace normal by half_vec, since microfacet aligned with half_vec will get reflected
func fresnel_schlick_appro(float3 specular_color, float3 l, float3 h) -> float3 {
    let loh = saturate(dot(l, h));
    let one_minus_loh = 1.0 - loh;
    let one_minus_loh_squared = one_minus_loh * one_minus_loh;
    return specular_color + (1.0 - specular_color) * one_minus_loh_squared * one_minus_loh_squared * one_minus_loh;
}

func fresnel_schlick_roughness_appro(float3 specular_color, float3 l, float3 h, float roughness) -> float3 {
    let loh = saturate(dot(l, h));
    let one_minus_loh = 1.0 - loh;
    let one_minus_loh_squared = one_minus_loh * one_minus_loh;
    return specular_color + (max(float3(1.0 - roughness), specular_color) - specular_color) * one_minus_loh_squared * one_minus_loh_squared * one_minus_loh;
}

// NDF (normal distribution function) describe the concentration of microfacet normals points in a given direction. (relative to surface area)
func ndf_ggx(float3 n, float3 h, float alpha2) -> float {
    const float noh = saturate(dot(n, h));
    const float denom = noh * noh * (alpha2 - 1.0) + 1.0;
    return alpha2 / (PI * denom * denom);
}

// Geometry function describe the chance that a microfacet with a given orientation is lit or visible.
func vis_smith_ggx(float nov, float nol, float alpha) -> float {
    let alpha2 = alpha * alpha;
    let lambda_v = nol * sqrt(nov * nov * (1.0 - alpha2) + alpha2);
    let lambda_l = nov * sqrt(nol * nol * (1.0 - alpha2) + alpha2);
    return 0.5 / max(lambda_v + lambda_l, 1e-7);
}

func vis_smith_joint(float3 v, float3 l, float3 n, float alpha) -> float {
    return vis_smith_ggx(saturate(dot(n, v)), saturate(dot(n, l)), alpha);
}

// Light is electromagnetic wave.
// In optical-flat surface, light will gets reflected and refracted inferred by maxwell equation.
// Specular term: reflection portion.
// Diffuse  term: refraction, scattering (comes from the light hits on a non-homogeneous medium), absorption, re-refraction portion.

// lambert model do NOT account for
// refraction (diffuse weaken on grazing angle) and
// roughness (diffuse reflectance average out microgeometry variation, but for larger scale it is NOT).
func diffuse_lambert(float3 diffuse_color) -> float3 {
    return diffuse_color * ONE_OVER_PI;
}

func brdf_lut_uv(float nov, float roughness) -> float2 {
    return float2(sqrt(saturate(nov)), roughness);
}

struct MicrofacetEnergy {
    // Single reflection energy for all outgoing directions
    float3 single_scattering;
    // Directional reflectance assign to the added specular lobe, already multiply by compensation. It is the total energy to redistribute
    float3 multiple_scattering;
    // Directional reflectance assign to diffuse, by a effective material parameter
    float3 diffuse;
    // Used to account for each bound's reflection energy lost during multi-scattering
    float3 compensation;
    // view-dependent factor added to diffuse brdf
    float3 diffuse_scale;
    // Average missing energy for unit-Fresnel single scattering
    float missing_average;

    static func new(float3 f0, float3 diffuse_color, float2 directional, float2 average) -> MicrofacetEnergy {
        MicrofacetEnergy energy;
        let average_albedo = saturate(average.x + average.y);
        energy.missing_average = 1.0 - average_albedo;
        let fresnel_average = f0 + (1.0 - f0) / 21.0;
        // Kulla–Conty with Hill's correction: Favg^2 * Eavg / (1 - Favg * (1 - Eavg)).
        energy.compensation = fresnel_average * fresnel_average * average_albedo
            / max(1.0 - fresnel_average * energy.missing_average, 1e-6);
        energy.single_scattering = f0 * directional.x + directional.y;
        // Ev = directional.x + directional.y, effective set the F0 to 1 to assume full reflectance
        energy.multiple_scattering = energy.compensation * (1.0 - saturate(directional.x + directional.y));
        energy.diffuse = diffuse_color * max(1.0 - energy.single_scattering - energy.multiple_scattering, 0.0);
        let average_specular = f0 * average.x + average.y + energy.compensation * energy.missing_average;
        energy.diffuse_scale = energy.diffuse / max(1.0 - average_specular, 1e-6);
        return energy;
    }

    func evaluate(float3 f0, float2 light) -> float3 {
        let missing_light = 1.0 - saturate(light.x + light.y);
        let specular_light = f0 * light.x + light.y + compensation * missing_light;
        // Normalized separable lobes integrate to the missing specular and remaining diffuse energy.
        return (multiple_scattering * missing_light / max(missing_average, 1e-6)
            + diffuse_scale * max(1.0 - specular_light, 0.0)) * ONE_OVER_PI;
    }

    func environment(float3 radiance, float3 diffuse_radiance, float ao) -> float3 {
        return radiance * single_scattering + diffuse_radiance * (multiple_scattering + diffuse * ao);
    }
};

func evaluate_microfacet_brdf(float3 n, float3 v, float3 l, float alpha, float3 f0,
    MicrofacetEnergy energy, float2 light_integral) -> float3 {
    if (dot(n, v) <= 0.0 || dot(n, l) <= 0.0 || dot(l + v, l + v) <= 1e-8) { return 0.0; }
    let h = normalize(l + v);
    let specular = fresnel_schlick_appro(f0, l, h) * ndf_ggx(n, h, alpha * alpha) * vis_smith_joint(v, l, n, alpha);
    return specular + energy.evaluate(f0, light_integral);
}
