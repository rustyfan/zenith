import neural_material_common;

#ifndef BACKEND
#define BACKEND 0
#endif
#ifndef GROUP_SIZE
#define GROUP_SIZE 32
#endif

struct Root {
    float* weights;
#if BACKEND == 0
    void* half_weights;
#else
    half* half_weights;
#endif
    void* packed0;
    void* packed1;
    void* packed2;
    float4* output;
    uint width;
    uint height;
};
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

float2 sample_uv(uint index) {
    return (float2(index % root.width, index / root.width) + 0.5) / float2(root.width, root.height);
}

#if BACKEND == 0
[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= root.width * root.height) { return; }
    float a[32];
    float b[32];
    float2 uv = sample_uv(id.x);
    for (uint row = 0; row < 32; row++) {
        float sum = root.weights[512 + row];
        for (uint col = 0; col < 16; col++) { sum += root.weights[row * 16 + col] * feature(uv, col); }
        a[row] = max(sum, 0.0);
    }
    for (uint row = 0; row < 32; row++) {
        float sum = root.weights[1568 + row];
        for (uint col = 0; col < 32; col++) { sum += root.weights[544 + row * 32 + col] * a[col]; }
        b[row] = max(sum, 0.0);
    }
    float4 result;
    for (uint row = 0; row < 4; row++) {
        float sum = root.weights[2112 + row];
        for (uint col = 0; col < 32; col++) { sum += root.weights[1600 + row * 32 + col] * b[col]; }
        result[row] = saturate(sum);
    }
    root.output[id.x] = result;
}
#elif BACKEND == 1
[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= root.width * root.height) { return; }
    CoopVec<half, 16> input;
    for (uint i = 0; i < 16; i++) { input[i] = half(feature(sample_uv(id.x), i)); }
    let a = max(coopVecMatMulAdd<half, 32, 16>(input, CoopVecComponentType.Float16,
        root.packed0, CoopVecComponentType.Float16, (void*)(root.half_weights + 512),
        CoopVecComponentType.Float16, CoopVecMatrixLayout.InferencingOptimal, false, 0), CoopVec<half, 32>(half(0.0)));
    let b = max(coopVecMatMulAdd<half, 32, 32>(a, CoopVecComponentType.Float16,
        root.packed1, CoopVecComponentType.Float16, (void*)(root.half_weights + 1568),
        CoopVecComponentType.Float16, CoopVecMatrixLayout.InferencingOptimal, false, 0), CoopVec<half, 32>(half(0.0)));
    let c = coopVecMatMulAdd<half, 16, 32>(b, CoopVecComponentType.Float16,
        root.packed2, CoopVecComponentType.Float16, (void*)(root.half_weights + 2112),
        CoopVecComponentType.Float16, CoopVecMatrixLayout.InferencingOptimal, false, 0);
    root.output[id.x] = saturate(float4(c[0], c[1], c[2], c[3]));
}
#else
using namespace linalg;
groupshared half activations[16 * 32];
groupshared float results[16 * 32];

#if BACKEND == 3
static const MemoryScope SCOPE = MemoryScope.Workgroup;
#else
static const MemoryScope SCOPE = MemoryScope.Subgroup;
#endif

void evaluate_layer<int INPUTS, int OUTPUTS>(uint lane, uint matrix_offset, uint bias_offset, bool activate) {
#if BACKEND == 3
    static const int TILE_N = OUTPUTS;
    static const int TILE_K = INPUTS;
#else
    static const int TILE_N = 16;
    static const int TILE_K = 16;
#endif
    typealias MatA = CoopMat<half, SCOPE, 16, TILE_K, CoopMatMatrixUse.MatrixA>;
    typealias MatB = CoopMat<half, SCOPE, TILE_K, TILE_N, CoopMatMatrixUse.MatrixB>;
    typealias MatC = CoopMat<float, SCOPE, 16, TILE_N, CoopMatMatrixUse.MatrixAccumulator>;
    for (uint tile = 0; tile < OUTPUTS; tile += TILE_N) {
        MatC sum = MatC(0.0);
        for (uint k = 0; k < INPUTS; k += TILE_K) {
            let a = MatA.Load<CoopMatMatrixLayout.RowMajor>(activations, k, 32);
            let b = MatB.Load<CoopMatMatrixLayout.ColumnMajor>(root.half_weights, matrix_offset + tile * INPUTS + k, INPUTS);
            sum = coopMatMulAdd<float, false>(a, b, sum);
        }
        sum.Store<CoopMatMatrixLayout.RowMajor>(results, tile, 32);
    }
    GroupMemoryBarrierWithGroupSync();
    for (uint i = lane; i < 16 * 32; i += GROUP_SIZE) {
        uint col = i % 32;
        float value = col < OUTPUTS ? results[i] + float(root.half_weights[bias_offset + col]) : 0.0;
        activations[i] = half(activate ? max(value, 0.0) : value);
    }
    GroupMemoryBarrierWithGroupSync();
}

[shader("compute")]
[numthreads(GROUP_SIZE, 1, 1)]
void main(uint3 group : SV_GroupID, uint lane : SV_GroupIndex) {
    for (uint i = lane; i < 16 * 32; i += GROUP_SIZE) {
        uint sample = group.x * 16 + i / 32;
        activations[i] = half(i % 32 < 16 && sample < root.width * root.height ? feature(sample_uv(sample), i % 32) : 0.0);
    }
    GroupMemoryBarrierWithGroupSync();
    evaluate_layer<16, 32>(lane, 0, 512, true);
    evaluate_layer<32, 32>(lane, 544, 1568, true);
    evaluate_layer<32, 16>(lane, 1600, 2112, false);
    for (uint row = lane; row < 16; row += GROUP_SIZE) {
        uint sample = group.x * 16 + row;
        if (sample < root.width * root.height) {
            uint offset = row * 32;
            root.output[sample] = saturate(float4(activations[offset], activations[offset + 1], activations[offset + 2], activations[offset + 3]));
        }
    }
}
#endif
