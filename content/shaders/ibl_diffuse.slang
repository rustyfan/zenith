import sh;
import basis;
import math_constants;

struct Root { float4* output; uint skybox; uint sampler; };
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;

static const uint SAMPLES = 1024;
[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 threadId : SV_DispatchThreadID)
{
    if (threadId.x > 0 || threadId.y > 0) {
        return;
    }

    ThreeBandSH sh3[3] = { ThreeBandSH(), ThreeBandSH(), ThreeBandSH() };
    TextureCube<float4> skybox = ResourceDescriptorHeap[NonUniformResourceIndex(root.skybox)];
    SamplerState sampler = SamplerDescriptorHeap[NonUniformResourceIndex(root.sampler)];

    let phi = PI * (sqrt(5.0) - 1.0);
    for (uint i = 0; i < SAMPLES; i++)
    {
        let y = 1.0 - ((float)i / (float)(SAMPLES)) * 2.0;
        let radius = sqrt(1.0 - y * y);

        let theta = phi * i;
        let x = cos(theta) * radius;
        let z = sin(theta) * radius;
        let dir = normalize(float3(x, y, z));

        let skybox_color = skybox.SampleLevel(sampler, dir, 0).rgb;

        sh3[0].project(dir, skybox_color.r);
        sh3[1].project(dir, skybox_color.g);
        sh3[2].project(dir, skybox_color.b);
    }

    let normalize_factor = 4 * PI / SAMPLES;

    sh3[0].multiply(normalize_factor);
    sh3[1].multiply(normalize_factor);
    sh3[2].multiply(normalize_factor);

    root.output[0] = sh3[0].coeff0;
    root.output[1] = sh3[0].coeff1;
    root.output[2] = float4(sh3[0].coeff2, 0.0, 0.0, 0.0);

    root.output[3] = sh3[1].coeff0;
    root.output[4] = sh3[1].coeff1;
    root.output[5] = float4(sh3[1].coeff2, 0.0, 0.0, 0.0);

    root.output[6] = sh3[2].coeff0;
    root.output[7] = sh3[2].coeff1;
    root.output[8] = float4(sh3[2].coeff2, 0.0, 0.0, 0.0);
}
