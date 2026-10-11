import brdf.brdf;
import brdf.ibl_sampling;
import common.math_constants;

struct BxdfSample {
    float3 direction;
    float pdf;
};

struct OpaqueBxdf {
    float3 n;
    float3 v;
    float roughness;
    float3 f0;
    MicrofacetEnergy energy;

    static func new(float3 n, float3 v, float roughness, float3 f0, float3 diffuse_color,
        float2 directional, float2 average) -> OpaqueBxdf {
        OpaqueBxdf bxdf;
        bxdf.n = n;
        bxdf.v = v;
        bxdf.roughness = clamp(roughness, MIN_ROUGHNESS, 1.0);
        bxdf.f0 = f0;
        bxdf.energy = MicrofacetEnergy.new(f0, diffuse_color, directional, average);
        return bxdf;
    }

    func eval(float3 l, float2 light_integral) -> float3 {
        return evaluate_microfacet_brdf(n, v, l, roughness * roughness, f0, energy, light_integral);
    }

    func diffuse_pdf(float3 l) -> float {
        return dot(n, v) > 0.0 ? saturate(dot(n, l)) * ONE_OVER_PI : 0.0;
    }

    func specular_pdf(float3 l) -> float {
        if (dot(n, v) <= 0.0 || dot(n, l) <= 0.0 || dot(l + v, l + v) <= 1e-8) { return 0.0; }
        let h = normalize(l + v);
        let noh = dot(n, h);
        let voh = dot(v, h);
        if (noh <= 0.0 || voh <= 0.0) { return 0.0; }
        let alpha = roughness * roughness;
        return ndf_ggx(n, h, alpha * alpha) * noh / (4.0 * voh);
    }

    func pdf(float3 l) -> float {
        return 0.5 * (diffuse_pdf(l) + specular_pdf(l));
    }

    func sample(float3 u) -> BxdfSample {
        BxdfSample result;
        result.direction = 0.0;
        result.pdf = 0.0;
        if (dot(n, v) <= 0.0) { return result; }
        let basis = build_orthonormal_basis(n);
        float3 l;
        if (u.z < 0.5) {
            let radius = sqrt(u.y);
            let phi = 2.0 * PI * u.x;
            l = mul(basis, float3(radius * cos(phi), radius * sin(phi), sqrt(1.0 - u.y)));
        } else {
            let h = mul(basis, sample_ggx(u.xy, roughness * roughness));
            if (dot(v, h) <= 0.0) { return result; }
            l = reflect(-v, h);
        }
        result.pdf = pdf(l);
        if (result.pdf > 0.0) { result.direction = l; }
        return result;
    }
};
