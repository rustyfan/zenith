import common.math_constants;
import brdf.hair_quadrature;

struct HairParameters {
    float3 absorption;
    float beta_m;
    float beta_n;
    float tilt;
    float eta;
    float opacity;
};

func hair_fresnel(float cosine, float eta) -> float {
    let c = saturate(cosine);
    let ct = sqrt(max(0.0, 1.0 - (1.0 - c * c) / (eta * eta)));
    let rs = (c - eta * ct) / max(c + eta * ct, 1e-8);
    let rp = (eta * c - ct) / max(eta * c + ct, 1e-8);
    return 0.5 * (rs * rs + rp * rp);
}

func hair_log_i0(float x) -> float {
    if (x > 12.0) {
        let inv = 1.0 / x;
        return x - 0.5 * log(2.0 * PI * x)
            + log(1.0 + inv * (0.125 + inv * (0.0703125 + inv * 0.0732421875)));
    }
    var term = 1.0;
    var sum = 1.0;
    for (uint k = 1; k <= 24; k++) {
        term *= x * x / (4.0 * float(k * k));
        sum += term;
    }
    return log(sum);
}

func hair_longitudinal(float ti, float to, float beta, uint model) -> float {
    let variance = max(beta * beta, 0.0001);
    if (model == 1) {
        let half_angle = (ti + to) * 0.5;
        let cd = cos((to - ti) * 0.5);
        return exp(-half_angle * half_angle / (2.0 * variance))
            / (sqrt(2.0 * PI * variance) * max(cd * cd, 1e-6));
    }
    // d'Eon 2011, Eq. 7, evaluated in log space to avoid overflow for narrow lobes.
    let a = abs(cos(ti) * cos(to)) / variance;
    let b = sin(ti) * sin(to) / variance;
    return exp(hair_log_i0(a) - b - 1.0 / variance)
        / (variance * (1.0 - exp(-2.0 / variance)));
}

func hair_wrapped_gaussian(float angle, float beta) -> float {
    let x = angle - 2.0 * PI * floor((angle + PI) / (2.0 * PI));
    let variance = beta * beta;
    var sum = 0.0;
    for (int k = -1; k <= 1; k++) {
        let d = x + float(k) * 2.0 * PI;
        sum += exp(-d * d / (2.0 * variance));
    }
    return sum / sqrt(2.0 * PI * variance);
}

func evaluate_hair_angles(HairParameters hair, float ti, float to, float phi,
    uint lobes, uint model, uint quality) -> float3 {
    let td = (to - ti) * 0.5;
    let cd = max(cos(td), 1e-4);
    let eta_prime = sqrt(max(hair.eta * hair.eta - sin(td) * sin(td), 0.0)) / cd;
    let cos_t = sqrt(max(1.0 - pow(sin(td) / hair.eta, 2.0), 1e-6));
    float3 azimuth[4] = { float3(0.0), float3(0.0), float3(0.0), float3(0.0) };
    let count = quality == 0 ? 16u : 64u;
    for (uint j = 0; j < count; j++) {
        let q = quality == 0 ? HAIR_QUADRATURE_16[j] : HAIR_QUADRATURE_64[j];
        let gi = asin(q.x);
        let gt = asin(clamp(q.x / eta_prime, -1.0, 1.0));
        let f = hair_fresnel(cd * cos(gi), hair.eta);
        // The unit-radius chord is 2*cos(gamma_t), not its square.
        let transmission = exp(-hair.absorption * (2.0 * cos(gt) / cos_t));
        var attenuation = float3(f);
        for (uint p = 0; p < 3; p++) {
            if ((lobes & (1u << p)) != 0) {
                let shift = 2.0 * float(p) * gt - 2.0 * gi + float(p) * PI;
                azimuth[p] += q.y * attenuation * hair_wrapped_gaussian(phi - shift, hair.beta_n);
            }
            attenuation = p == 0 ? (1.0 - f) * (1.0 - f) * transmission : attenuation * f * transmission;
        }
        if (model == 0 && (lobes & 8) != 0) {
            // Higher internal orders are collected in a normalized uniform azimuthal lobe.
            azimuth[3] += q.y * attenuation / max(1.0 - f * transmission, float3(1e-6)) / (2.0 * PI);
        }
    }
    float shifts[4] = { -2.0 * hair.tilt, hair.tilt, 4.0 * hair.tilt, 0.0 };
    float widths[4] = { hair.beta_m, hair.beta_m * 0.5, hair.beta_m * 2.0, hair.beta_m * 2.0 };
    var result = float3(0.0);
    for (uint p = 0; p < 4; p++) {
        result += azimuth[p] * hair_longitudinal(ti, to - shifts[p], widths[p], model);
    }
    return result;
}

func evaluate_hair(HairParameters hair, float3 tangent, float3 v, float3 l,
    uint lobes, uint model, uint quality) -> float3 {
    let si = clamp(dot(tangent, l), -0.999999, 0.999999);
    let so = clamp(dot(tangent, v), -0.999999, 0.999999);
    let lp = l - tangent * si;
    let vp = v - tangent * so;
    let phi = atan2(dot(tangent, cross(lp, vp)), dot(lp, vp));
    let result = evaluate_hair_angles(hair, asin(si), asin(so), phi, lobes, model, quality);
    return model == 1 ? result * sqrt(1.0 - si * si) : result;
}
