/// Zenith use right-hand z-up cartesian coordinate.
/// See camera.rs for more details.

func unit_spherical_to_cartesian(float theta, float phi) -> float3 {
    let sinTheta = sin(theta);
    return float3(
        sinTheta * cos(phi),
        sinTheta * sin(phi),
        cos(theta)
    );
}
