import brdf.ibl_sampling;
import debug.debug;
import shading.surface;
import shading.opaque;
import shading.shading_data;
import brdf.clearcoat;

func shade_clearcoat(SurfaceInput surface, SurfaceLighting lighting, float4 gbufferd) -> ShadingResult {
    var result = shade_opaque(surface, lighting);
    let coat = CoatData.from_gbuffer(gbufferd);
    let n = coat.normal;
    let weight = saturate(coat.weight);
    let roughness = clamp(coat.roughness, MIN_ROUGHNESS, 1.0);
    let integral = surface_integral(lighting, saturate(dot(n, surface.v)), roughness);
    let view_transmission = coat_transmission(weight, integral);
    result.indirect *= view_transmission * coat_transmission(weight, surface_average(lighting, roughness));
    let l = lighting.direction_to_light;
    if (!lighting.furnace) {
        result.direct *= view_transmission * coat_transmission(weight,
            surface_integral(lighting, saturate(dot(n, l)), roughness));
    }
    if ((lighting.debug_mode & DebugMode.BaseOnly) == 0 && weight > 0.0) {
        result.indirect += surface_reflection(lighting, n, surface.v, roughness) * weight * coat_albedo(integral);
        if (!lighting.furnace) {
            result.direct += weight * evaluate_clearcoat(n, surface.v, l, roughness)
                * saturate(dot(n, l)) * lighting.light_radiance;
        }
    }
    return result;
}
