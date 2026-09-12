struct Source { float4x4 matrix; float4 vector; float values[4]; };
struct Child { Source* source; float4* result; };
struct Producer { Child* child; Source* source; float4* result; uint* indirect; };
[[vk::binding(0,0)]] ConstantBuffer<Producer> producer;
[[vk::binding(0,0)]] ConstantBuffer<Child> child;
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
    child.result[0] = mul(child.source->matrix, child.source->vector) + child.source->values[2];
}
