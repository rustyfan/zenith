import zenith_bindings;
#ifdef ZENITH_CAPTURE
struct ZenithRoots { Root* vertex; Root* fragment; };
[[vk::push_constant]] ConstantBuffer<ZenithRoots> zenith_roots;
#ifdef ZENITH_FRAGMENT
#define root (*zenith_roots.fragment)
#else
#define root (*zenith_roots.vertex)
#endif
#else
[[vk::binding(0, 0)]] ConstantBuffer<Root> root;
#endif
