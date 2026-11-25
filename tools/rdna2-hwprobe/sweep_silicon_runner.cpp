#include <hip/hip_runtime.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
#define CK(x) do { hipError_t e = (x); if (e != hipSuccess) { fprintf(stderr, "%s: %s\n", #x, hipGetErrorString(e)); exit(1); } } while (0)
static std::vector<unsigned char> rd(const char* p) {
  FILE* f = fopen(p, "rb"); if (!f) { fprintf(stderr, "missing %s\n", p); exit(1); }
  std::vector<unsigned char> v; int c; while ((c = fgetc(f)) != EOF) v.push_back((unsigned char)c); fclose(f); return v;
}
int main(int argc, char** argv) {
  if (argc < 4 || (argc - 2) % 2) { fprintf(stderr, "usage: sweep_silicon_runner <k.co> (<in> <out>)...\n"); return 2; }
  hipModule_t mod; CK(hipModuleLoad(&mod, argv[1]));
  hipFunction_t fn; CK(hipModuleGetFunction(&fn, mod, "k"));
  for (int a = 2; a < argc; a += 2) {
    auto in = rd(argv[a]);
    void *data, *tab; CK(hipMalloc(&data, in.size())); CK(hipMalloc(&tab, 16));
    CK(hipMemcpy(data, in.data(), in.size(), hipMemcpyHostToDevice));
    unsigned long long va = (unsigned long long)data;
    unsigned w3 = 4u | (5u << 3) | (6u << 6) | (7u << 9) | (22u << 12) | (1u << 24) | (3u << 28);
    unsigned vs[4] = { (unsigned)va, (unsigned)(va >> 32) & 0xffffu, (unsigned)in.size(), w3 };
    CK(hipMemcpy(tab, vs, 16, hipMemcpyHostToDevice));
    struct { void* tab; } args = { tab };
    size_t asz = sizeof(args);
    void* cfg[] = { HIP_LAUNCH_PARAM_BUFFER_POINTER, &args, HIP_LAUNCH_PARAM_BUFFER_SIZE, &asz, HIP_LAUNCH_PARAM_END };
    CK(hipModuleLaunchKernel(fn, 32, 1, 1, 256, 1, 1, 0, nullptr, nullptr, cfg));
    CK(hipDeviceSynchronize());
    std::vector<unsigned char> out(in.size()); CK(hipMemcpy(out.data(), data, in.size(), hipMemcpyDeviceToHost));
    FILE* o = fopen(argv[a + 1], "wb"); fwrite(out.data(), 1, out.size(), o); fclose(o);
    CK(hipFree(data)); CK(hipFree(tab));
  }
  return 0;
}
