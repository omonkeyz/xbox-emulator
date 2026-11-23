#include <hip/hip_runtime.h>
#include <cstdio>
#include <cstdlib>
#include <vector>

#define CK(x) do { hipError_t e = (x); if (e != hipSuccess) { fprintf(stderr, "%s: %s\n", #x, hipGetErrorString(e)); exit(1); } } while (0)

int main(int argc, char** argv) {
  if (argc < 5) { fprintf(stderr, "usage: verify_runner <k.co> <kernel> <in.bin> <out.bin> [cases] [out bytes per case]\n"); return 2; }
  FILE* f = fopen(argv[3], "rb");
  if (!f) { fprintf(stderr, "missing %s\n", argv[3]); return 1; }
  std::vector<unsigned> in;
  unsigned w;
  while (fread(&w, 4, 1, f) == 1) in.push_back(w);
  fclose(f);
  size_t n = argc > 5 ? strtoul(argv[5], 0, 0) : in.size() / 4;
  size_t per = argc > 6 ? strtoul(argv[6], 0, 0) : 16;
  hipModule_t mod; hipFunction_t fn;
  CK(hipModuleLoad(&mod, argv[1]));
  CK(hipModuleGetFunction(&fn, mod, argv[2]));
  void *din, *dout;
  CK(hipMalloc(&din, in.size() * 4));
  CK(hipMalloc(&dout, n * per));
  CK(hipMemcpy(din, in.data(), in.size() * 4, hipMemcpyHostToDevice));
  CK(hipMemset(dout, 0xAA, n * per));
  struct { void* in; void* out; } args = { din, dout };
  size_t asz = sizeof(args);
  void* cfg[] = { HIP_LAUNCH_PARAM_BUFFER_POINTER, &args, HIP_LAUNCH_PARAM_BUFFER_SIZE, &asz, HIP_LAUNCH_PARAM_END };
  CK(hipModuleLaunchKernel(fn, (unsigned)n, 1, 1, 32, 1, 1, 0, nullptr, nullptr, cfg));
  CK(hipDeviceSynchronize());
  std::vector<unsigned> out(n * per / 4);
  CK(hipMemcpy(out.data(), dout, n * per, hipMemcpyDeviceToHost));
  FILE* o = fopen(argv[4], "wb");
  fwrite(out.data(), 4, out.size(), o);
  fclose(o);
  return 0;
}
