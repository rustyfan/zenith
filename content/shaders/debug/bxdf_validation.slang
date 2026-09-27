import brdf.hair_marschner;
import brdf.clearcoat;

struct BxdfCase {
    HairParameters hair;
    float4 angles;
    float4 surface;
};
func validate_bxdf(BxdfCase c) -> float4 {
    let n = float3(0.0, 0.0, 1.0);
    let v = float3(sqrt(max(0.0, 1.0 - c.surface.x * c.surface.x)), 0.0, c.surface.x);
    let l = float3(0.0, sqrt(max(0.0, 1.0 - c.surface.y * c.surface.y)), c.surface.y);
    return float4(evaluate_hair_angles(c.hair, c.angles.x, c.angles.y, c.angles.z, 15, uint(c.angles.w), 1),
        evaluate_clearcoat(n, v, l, c.surface.z));
}
