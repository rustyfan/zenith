import debug.bxdf_validation;
struct Root { BxdfCase* cases; float4* output; uint count; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x < root.count) { root.output[id.x] = validate_bxdf(root.cases[id.x]); }
}
