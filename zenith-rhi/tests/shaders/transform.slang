struct Root
{
    uint* input;
    uint* output;
    uint count;
    uint multiplier;
};
#include "zenith_root.slang"

[shader("compute")]
[numthreads(64, 1, 1)]
void transform(uint3 id : SV_DispatchThreadID)
{
    if (id.x < root.count)
        root.output[id.x] = root.input[id.x] * root.multiplier + id.x;
}
