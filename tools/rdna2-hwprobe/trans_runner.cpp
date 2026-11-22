#include <hip/hip_runtime.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
#define CK(x) do { hipError_t e = (x); if (e != hipSuccess) { fprintf(stderr, "%s:%d %s: %s\n", __FILE__, __LINE__, #x, hipGetErrorString(e)); return 1; } } while (0)
int main(int argc, char** argv) {
  if (argc < 7) { fprintf(stderr, "usage: runner <k.co> <kernel> <count> <base> <stride> <out.bin> [words_per_thread]\n"); return 2; }
  const char* co = argv[1]; const char* kn = argv[2];
  unsigned long long count = strtoull(argv[3], 0, 0);
  unsigned base = strtoul(argv[4], 0, 0), stride = strtoul(argv[5], 0, 0);
  unsigned wpt = argc > 7 ? strtoul(argv[7], 0, 0) : 1;
  if (count % 256) { fprintf(stderr, "count must be a multiple of 256\n"); return 2; }
  CK(hipInit(0));
  hipModule_t mod; CK(hipModuleLoad(&mod, co));
  hipFunction_t fn; CK(hipModuleGetFunction(&fn, mod, kn));
  size_t outsz = count * 4ull * wpt;
  void* dout; CK(hipMalloc(&dout, outsz));
  CK(hipMemset(dout, 0xAA, outsz));
  struct { void* out; unsigned base; unsigned stride; } args = { dout, base, stride };
  size_t asz = sizeof(args);
  void* cfg[] = { HIP_LAUNCH_PARAM_BUFFER_POINTER, &args, HIP_LAUNCH_PARAM_BUFFER_SIZE, &asz, HIP_LAUNCH_PARAM_END };
  unsigned long long groups = count / 256, done = 0;
  while (done < groups) {
    unsigned long long g = groups - done; if (g > 65535ull * 256) g = 65535ull * 256;
    args.base = base + (unsigned)(done * 256ull * stride);
    void* off = (char*)dout + done * 256ull * 4ull * wpt; args.out = off;
    CK(hipModuleLaunchKernel(fn, (unsigned)g, 1, 1, 256, 1, 1, 0, nullptr, nullptr, cfg));
    CK(hipDeviceSynchronize());
    done += g;
  }
  std::vector<char> out(outsz); CK(hipMemcpy(out.data(), dout, outsz, hipMemcpyDeviceToHost));
  FILE* f = fopen(argv[6], "wb"); fwrite(out.data(), 1, outsz, f); fclose(f);
  CK(hipFree(dout));
  printf("%s: %llu threads\n", kn, count);
  return 0;
}
