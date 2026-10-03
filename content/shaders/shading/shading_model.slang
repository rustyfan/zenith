enum ShadingModel : uint { Opaque = 0, ClearCoat = 1, Hair = 2 };
func shading_model_color(ShadingModel model) -> float3 {
    switch (model) {
        case ShadingModel.Opaque: return float3(1, 0, 0);
        case ShadingModel.ClearCoat: return float3(0, 1, 0);
        case ShadingModel.Hair: return float3(0, 0, 1);
        default: return float3(1, 0, 1);
    }
}
