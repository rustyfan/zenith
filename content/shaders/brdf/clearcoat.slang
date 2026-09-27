import brdf.brdf;

func coat_albedo(float2 integral) -> float {
    return saturate(0.04 * integral.x + integral.y);
}

func coat_transmission(float weight, float2 integral) -> float {
    return 1.0 - weight * coat_albedo(integral);
}

func evaluate_clearcoat(float3 n, float3 v, float3 l, float roughness) -> float {
    let nv = dot(n, v);
    let nl = dot(n, l);
    if (nv <= 0.0 || nl <= 0.0 || dot(v + l, v + l) < 1e-8) { return 0.0; }
    let h = normalize(v + l);
    let alpha = roughness * roughness;
    return fresnel_schlick_appro(float3(0.04), l, h).x
        * ndf_ggx(n, h, alpha * alpha) * vis_smith_ggx(nv, nl, alpha);
}
