public typedef uint tex;
public typedef uint sampler;

#ifdef ZENITH_CAPTURE
[[vk::binding(0, 0)]] __DynamicResource<__DynamicResourceKind.Sampler> zenith_samplers[];
[[vk::binding(2, 0)]] __DynamicResource<__DynamicResourceKind.General> zenith_sampled[];
[[vk::binding(3, 0)]] __DynamicResource<__DynamicResourceKind.General> zenith_storage[];
export T getDescriptorFromHandle<T>(DescriptorHandle<T> handle) where T : IOpaqueDescriptor {
    uint index = NonUniformResourceIndex(((uint2)handle).x);
    if (T.kind == DescriptorKind.Sampler)
        return zenith_samplers[index].asOpaqueDescriptor<T>();
    else if (T.descriptorAccess == DescriptorAccess.Read)
        return zenith_sampled[index].asOpaqueDescriptor<T>();
    else
        return zenith_storage[index].asOpaqueDescriptor<T>();
}
#endif
