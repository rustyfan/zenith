struct Source { float4x4 matrix; float4 vector; float values[4]; };
struct Child { Source* source; float4* result; };
struct Producer { Child* child; Source* source; float4* result; uint* indirect; };
#ifdef ZENITH_CAPTURE
struct Root { uint64_t address; uint64_t fragment; };
[[vk::push_constant]] ConstantBuffer<Root> roots;
#define producer (*(Producer*)roots.address)
#define child_root (*(Child*)roots.address)
#else
[[vk::binding(0,0)]] ConstantBuffer<Producer> producer;
[[vk::binding(0,0)]] ConstantBuffer<Child> child_root;
#endif
[shader("compute")]
[numthreads(1,1,1)]
void produce()
{
    producer.child->source = producer.source;
    producer.child->result = producer.result;
    producer.indirect[0] = 1; producer.indirect[1] = 1; producer.indirect[2] = 1;
}
[shader("compute")]
[numthreads(1,1,1)]
void consume()
{
    child_root.result[0] = mul(child_root.source->matrix, child_root.source->vector) + child_root.source->values[2];
}
