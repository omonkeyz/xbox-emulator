#include <cmath>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <vector>
#include <numeric>
#include <algorithm>
typedef long double LD;
typedef __int128 I128;
static void lsolve(std::vector<std::vector<LD>> A, std::vector<LD> b, std::vector<LD>& x) {
  int n = b.size();
  for (int c = 0; c < n; c++) { int p = c; for (int r = c + 1; r < n; r++) if (fabsl(A[r][c]) > fabsl(A[p][c])) p = r; std::swap(A[c], A[p]); std::swap(b[c], b[p]);
    for (int r = 0; r < n; r++) if (r != c) { LD m = A[r][c] / A[c][c]; for (int k = c; k < n; k++) A[r][k] -= m * A[c][k]; b[r] -= m * b[c]; } }
  x.resize(n); for (int i = 0; i < n; i++) x[i] = b[i] / A[i][i];
}
static inline I128 fdiv(I128 v, I128 d) { return v >= 0 ? v / d : -((-v + d - 1) / d); }
static inline int64_t fl(int64_t v, int64_t d) { return v >= 0 ? v / d : -((-v + d - 1) / d); }
int SB = 18, SQ = 1;
static inline int64_t Sof(uint32_t t) { uint64_t a = t >> SQ; return (int64_t)((a * a) >> 10); }
int TIEC = 0, TIEF = 0; int64_t QC = 64, QLO = 32, QHI = 64, TPB = 29, FSB = 24;
static inline I128 rnd(I128 v, I128 d, int mode) { I128 q = fdiv(v, d), r = v - q * d; I128 h = d / 2; if (r > h) return q + 1; if (r < h) return q; if (mode == 0) return q + 1; if (mode == 1) return q; return (q & 1) ? q + 1 : q; }
static inline int64_t coarseOf(int64_t c0, int64_t N1, uint32_t t) {
  I128 P = (I128)N1 * t; I128 mag = P < 0 ? -P : P; int64_t q = mag < ((I128)1 << (TPB + SB)) ? QLO : QHI;
  I128 Pq = rnd(P, (I128)q << SB, TIEC) * q;
  return (int64_t)rnd((I128)c0 + Pq, QC, TIEC) * QC;
}
struct P { int64_t c0, N1, Dn, Q, M, phi, c2; };
static inline int64_t model(const P& p, uint32_t t) {
  int64_t H = p.c2 + p.Q * fl(p.M * (int64_t)t + p.phi, p.Dn);
  I128 v = (I128)H * Sof(t);
  I128 vm = v < 0 ? -v : v; int sh = 24; while (vm >= ((I128)1 << (sh + FSB))) sh++;
  return coarseOf(p.c0, p.N1, t) + (int64_t)(rnd(v, (I128)1 << sh, TIEF) << (sh - 24));
}
std::vector<int64_t> Y;
uint32_t SEG = 1 << 18;
int main(int argc, char** argv) {
  Y.resize(1 << 23);
  const char* OP = getenv("OP") ? getenv("OP") : "rcp"; int EX = getenv("EX") ? atoi(getenv("EX")) : 0; int SC = getenv("SC") ? atoi(getenv("SC")) : 0;
  if (getenv("SB")) { SB = atoi(getenv("SB")); SEG = 1u << SB; } if (getenv("SQ")) SQ = atoi(getenv("SQ"));
  if (SC == 1) { QC = 128; QLO = 64; QHI = 128; TPB = 30; FSB = 25; }
  if (SC == 2) { QC = 128; QLO = 32; QHI = 64; TPB = 29; FSB = 24; }
  if (SC == 4) { QC = 64; QLO = 64; QHI = 128; TPB = 30; FSB = 24; }
  if (SC == 5) { QC = 64; QLO = 64; QHI = 128; TPB = 29; FSB = 24; }
  if (SC == 6) { QC = 64; QLO = 64; QHI = 128; TPB = 31; FSB = 24; }
  if (SC == 3) { QC = 128; QLO = 32; QHI = 64; TPB = 29; FSB = 25; }
  int keep = strcmp(OP, "rcp") == 0 ? 18 : 17; int eref = strcmp(OP, "sqrt") == 0 ? 0 : -1; if (getenv("EREF")) eref = atoi(getenv("EREF"));
  for (int k = 0; k < 8; k++) {
    char p[64]; if (strcmp(OP, "rcp") == 0) snprintf(p, 64, "data/rcp64_k%d.bin", k); else snprintf(p, 64, "data/%s64_e%d_k%d.bin", OP, EX, k);
    FILE* g = fopen(p, "rb"); std::vector<uint32_t> w(2 << 20); if (fread(w.data(), 4, 2 << 20, g) != (2u << 20)) return 2; fclose(g);
    for (uint32_t i = 0; i < (1u << 20); i++) { uint64_t dd = ((uint64_t)w[2 * i + 1] << 32) | w[2 * i]; int e = (int)((dd >> 52) & 0x7ff) - 1023; uint64_t man = (dd & 0xfffffffffffffULL) | (1ull << 52); Y[(i << 3) | k] = (int64_t)((man >> keep) << (e - eref)); }
  }
  TIEC = atoi(argv[3]); TIEF = atoi(argv[4]);
  FILE* out = fopen(argc > 5 ? argv[5] : "rcp64_tables.txt", "w");
  for (int si = atoi(argv[1]); si <= atoi(argv[2]); si++) {
    uint32_t first = si == 0;
    std::vector<std::vector<LD>> A(4, std::vector<LD>(4, 0)); std::vector<LD> b(4, 0), cf, v(4);
    for (uint32_t t = first; t < SEG; t++) { LD S = ldexpl((LD)Sof(t), -24), u = (LD)t / SEG; v[0] = 1; v[1] = u; v[2] = S; v[3] = S * u; for (int i = 0; i < 4; i++) { for (int j = 0; j < 4; j++) A[i][j] += v[i] * v[j]; b[i] += v[i] * Y[si * SEG + t]; } }
    lsolve(A, b, cf);
    std::vector<int64_t> CO(SEG); std::vector<uint32_t> idx;
    LD sx = 0, sy = 0;
    for (uint32_t t = first; t < 65536; t++) { LD S = ldexpl((LD)Sof(t), -24), u = (LD)t / SEG; LD r = (LD)Y[si * SEG + t] - (cf[2] * S + cf[3] * S * u); LD ang = 2 * M_PIl * (r / (LD)QC - floorl(r / (LD)QC)); sx += cosl(ang); sy += sinl(ang); }
    LD shift = atan2l(sy, sx) / (2 * M_PIl) * (LD)QC;
    for (uint32_t t = first; t < SEG; t++) { LD S = ldexpl((LD)Sof(t), -24), u = (LD)t / SEG; LD rem = ((LD)Y[si * SEG + t] - (cf[2] * S + cf[3] * S * u) - shift) / (LD)QC; int64_t c = llroundl(rem); CO[t] = c * QC; if (fabsl(rem - c) < 0.3L && t < 65536) idx.push_back(t); }
    if (idx.size() < 1000) { printf("seg %2d: too few unambiguous points (%zu)\n", si, idx.size()); fflush(stdout); continue; }
    std::vector<uint32_t> ord = idx; for (size_t i = 0; i < ord.size(); i++) std::swap(ord[i], ord[(i * 2654435761ull) % ord.size()]);
    int64_t Nc = llroundl(cf[1]), y0 = (llroundl(cf[0]) / QC) * QC;
    int64_t bN = 0, bc = 0; bool cf_ok = false;
    for (int64_t d = 0; d <= 3000 && !cf_ok; d++) for (int sgn : {1, -1}) {
      int64_t N1 = Nc + sgn * d; if (d == 0 && sgn == -1) continue; if (getenv("MUL") && N1 % atoi(getenv("MUL"))) continue;
      for (int64_t c0 = y0 - 256; c0 <= y0 + 256 && !cf_ok; c0++) {
        bool good = true; int n = 0;
        for (uint32_t t : ord) { if (coarseOf(c0, N1, t) != CO[t]) { good = false; break; } if (++n > 4000) break; }
        if (!good) continue;
        long bad = 0; for (uint32_t t : idx) if (coarseOf(c0, N1, t) != CO[t]) bad++;
        if (bad == 0) { cf_ok = true; bN = N1; bc = c0; }
      }
      if (cf_ok) break;
    }
    if (!cf_ok && getenv("AMB")) { long h[8] = {0}; for (uint32_t t = first; t < SEG; t++) { LD S = ldexpl((LD)Sof(t), -24), u = (LD)t / SEG; LD rem = ((LD)Y[si * SEG + t] - (cf[2] * S + cf[3] * S * u)) / 64.0L; LD fr = rem - floorl(rem); h[(int)(fr * 8)]++; } printf("  rem fraction histogram:"); for (int i = 0; i < 8; i++) printf(" %ld", h[i]); printf("\n"); long bb[16] = {0}; for (uint32_t t = first; t < SEG; t++) { LD S = ldexpl((LD)Sof(t), -24), u = (LD)t / SEG; LD rem = ((LD)Y[si * SEG + t] - (cf[2] * S + cf[3] * S * u)) / 64.0L; if (fabsl(rem - llroundl(rem)) >= 0.3L) bb[t >> 14]++; } printf("  ambiguous by 1/16:"); for (int i = 0; i < 16; i++) printf(" %ld", bb[i]); printf("\n"); }
    if (!cf_ok) { long amb = (long)(SEG - first) - (long)idx.size(); printf("seg %2d: coarse not found (LSQ c1 %lld, %ld ambiguous points)\n", si, (long long)Nc, amb); fprintf(out, "%d FAIL coarse\n", si); fflush(stdout); continue; }
    std::vector<uint32_t> ts; std::vector<int64_t> Hs; int64_t Q = 0; LD c3step = cf[3] / SEG; P best{}; long bestbad = 1L << 40; bool found = false;
    for (int64_t G : {1, 2, 4}) {
      if (found) break;
      ts.clear(); Hs.clear();
      for (uint32_t t = 150000u << (SB - 18); t < (150000u << (SB - 18)) + 6000; t++) {
        int64_t R = Y[si * SEG + t] - coarseOf(bc, bN, t), S = Sof(t);
        I128 l2 = ((I128)R << 24) - ((I128)1 << 23), h2 = ((I128)R << 24) + ((I128)1 << 23);
        int64_t hmin = (int64_t)fdiv(l2 + (I128)S * G - 1, (I128)S * G), hmax = (int64_t)fdiv(h2, (I128)S * G);
        if (hmin == hmax) { ts.push_back(t); Hs.push_back(hmin * G); }
      }
      Q = 0; for (size_t i = 1; i < Hs.size(); i++) Q = std::gcd(Q, std::llabs(Hs[i] - Hs[i - 1]));
      if (ts.size() < 100 || !Q) continue;
      int64_t Q0 = Q; for (int64_t Qc : {Q0, (int64_t)2, (int64_t)1}) { if (found) break; Q = Qc; if (Q0 % Q) continue;
      for (int64_t Dn = 64; Dn <= 4096 && !found; Dn *= 2) {
        int64_t M0 = llroundl(c3step * Dn / Q);
        int64_t mr = 2 + Dn / 512;
        for (int64_t M = M0 - mr; M <= M0 + mr && !found; M++) for (int64_t phi = 0; phi < Dn && !found; phi++) {
          int64_t c2 = Hs[0] - Q * fl(M * (int64_t)ts[0] + phi, Dn); bool okk = true;
          for (size_t i = 1; i < ts.size(); i++) if (Hs[i] - Q * fl(M * (int64_t)ts[i] + phi, Dn) != c2) { okk = false; break; }
          if (!okk) continue;
          P p{bc, bN, Dn, Q, M, phi, c2};
          long bad = 0; for (uint32_t t = first; t < SEG; t++) if (Y[si * SEG + t] != model(p, t) && ++bad >= bestbad) break;
          if (bad < bestbad) { bestbad = bad; best = p; }
          if (!bad) found = true;
        }
      }
    }
      }

    if (!best.Dn) { printf("seg %2d: fine not found (coarse c0 %lld c1 %lld; Q %lld from %zu exact H points; H range %lld..%lld)\n", si, (long long)bc, (long long)bN, (long long)Q, ts.size(), ts.empty() ? 0LL : (long long)Hs.front(), ts.empty() ? 0LL : (long long)Hs.back()); fflush(stdout); fprintf(out, "%d FAIL fine\n", si); continue; }
    { int shown = 0; for (uint32_t t = first; t < SEG && shown < 6; t++) if (Y[si * SEG + t] != model(best, t)) { printf("   mismatch t=%u Y=%lld model=%lld diff=%lld S=%lld\n", t, (long long)Y[si * SEG + t], (long long)model(best, t), (long long)(Y[si * SEG + t] - model(best, t)), (long long)Sof(t)); shown++; } }
    printf("seg %2d: mismatches %ld  c0 %lld c1 %lld  H = %lld + %lld*floor((%lld t + %lld)/%lld)\n", si, bestbad, (long long)best.c0, (long long)best.N1, (long long)best.c2, (long long)best.Q, (long long)best.M, (long long)best.phi, (long long)best.Dn);
    fprintf(out, "%d %ld %lld %lld %lld %lld %lld %lld %lld\n", si, bestbad, (long long)best.c0, (long long)best.N1, (long long)best.c2, (long long)best.Q, (long long)best.M, (long long)best.phi, (long long)best.Dn);
    fflush(stdout); fflush(out);
  }
}
