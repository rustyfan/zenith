import sh;
import math_constants;

struct Root { float4* output; uint skybox; uint sampler; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

static const uint SAMPLES = 1024;
static const float GOLDEN_ANGLE = 2.399963229728653322;

[shader("compute")]
[numthreads(1, 1, 1)]
void main(uint3 threadId : SV_DispatchThreadID) {
    ThreeBandSH sh3[3] = { ThreeBandSH(), ThreeBandSH(), ThreeBandSH() };
    TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];
    for (uint i = 0; i < SAMPLES; i++) {
        let theta = float(i) * GOLDEN_ANGLE;
        let z = 1.0 - 2.0 * (float(i) + 0.36) / float(SAMPLES);
        let radius = sqrt(1.0 - z * z);
        let dir = float3(cos(theta) * radius, sin(theta) * radius, z);
        let radiance = skybox.SampleLevel(sampler, dir, 0).rgb;
        sh3[0].project(dir, radiance.r);
        sh3[1].project(dir, radiance.g);
        sh3[2].project(dir, radiance.b);
    }

    // Stashed SH convolution (SH Tricks, A10): basis and cosine kernel include Lambert's 1/pi.
    let c0 = 0.282094791773;
    let c1 = 0.325735007935;
    let c2 = 0.273137107648;
    let c3 = 0.078847891313;
    let c4 = 0.136568553824;
    for (uint channel = 0; channel < 3; channel++) {
        sh3[channel].multiply(4.0 * PI / float(SAMPLES));
        let sh = sh3[channel];
        root.output[channel] = float4(-c1 * sh.coeff0.w, -c1 * sh.coeff0.y, c1 * sh.coeff0.z, c0 * sh.coeff0.x - c3 * sh.coeff1.z);
        root.output[channel + 3] = float4(c2 * sh.coeff1.x, -c2 * sh.coeff1.y, 3.0 * c3 * sh.coeff1.z, -c2 * sh.coeff1.w);
    }
    root.output[6] = float4(c4 * sh3[0].coeff2, c4 * sh3[1].coeff2, c4 * sh3[2].coeff2, 0.0);
}
