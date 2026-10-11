import brdf.ibl_sampling;
import debug.debug;
import shading.sh;
import shading.surface;
import brdf.opaque;

func shade_opaque(SurfaceInput surface, SurfaceLighting lighting) -> ShadingResult {
    let n = surface.n;
    let v = surface.v;
    let nov = saturate(dot(n, v));
    let roughness = clamp(surface.roughness, MIN_ROUGHNESS, 1.0);
    let metallic = saturate(surface.metallic);
    let base = lighting.furnace ? float3(1.0) : surface.base;
    let specular_color = lerp(float3(0.04), base, metallic);
    let diffuse_color = lighting.furnace ? float3(1.0) : (1.0 - metallic) * base;
    let integrated = surface_integral(lighting, nov, roughness);
    let average = surface_average(lighting, roughness);
    var bxdf = OpaqueBxdf.new(n, v, roughness, specular_color, diffuse_color, integrated, average);
    if (!lighting.multiple_scattering) { bxdf.energy.multiple_scattering = 0.0; }
    ShadingResult result;
    result.direct = 0.0;
    result.indirect = 0.0;
    if ((lighting.debug_mode & DebugMode.CoatOnly) != 0) { return result; }
    if (lighting.furnace) {
        result.indirect = bxdf.energy.environment(float3(1.0), float3(1.0), 1.0);
        return result;
    }
    let l = lighting.direction_to_light;
    let nol = saturate(dot(n, l));
    if (nol > 0.0 && nov > 0.0 && dot(l + v, l + v) > 1e-8) {
        result.direct = bxdf.eval(l, surface_integral(lighting, nol, roughness)) * lighting.light_radiance * nol;
    }
    result.indirect = bxdf.energy.environment(surface_reflection(lighting, n, v, roughness), evaluate_diffuse_sh(lighting.sh, n), surface.ao);
    return result;
}
