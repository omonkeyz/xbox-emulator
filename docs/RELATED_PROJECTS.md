# Related projects

Other public work on running Xbox One / Series X|S titles (or parts of that stack) on PC, checked on 2026-11-21 for pieces this emulator can use. "Copy" means the licence is compatible with MIT OR Apache-2.0 (keep the notice); "reference" means behaviour only, no code.

## Directly transferable

| Project | Licence | What helps here | Area |
|---|---|---|---|
| [brcly/TomBox](https://github.com/brcly/TomBox) ("TuxBox", fork of TomBox, 45 commits ahead of 9969cff) | crates declare MIT OR Apache-2.0 (no root LICENSE) | 16/64-bit and class_f16 VOPC decode + lowering (`crates/tuxbox-xshader/src/rdna2/mnem.rs`, `hlsl/compare.rs`); 190 VOPC mnemonics checked on a real RDNA2 GPU (`tools/progress/native-vopc.tsv`, harness `tools/homebrew/native_vopc.py`); deterministic loop-head emission (2c729ef); buffer-only read states (0x3) dropped from texture barriers and initial states (dde663f, `d3d12_xs/src/barriers.rs`); Map of DEFAULT-heap resources returns a CPU shadow (bfc2947); per-caller GXDK edition routing of command-list slots 59/61 (483bb34, `gxdk_edition.rs`); interpolation modifiers copied from PS inputs to VS/GS outputs (6b49c14); ISA table check against AMD's machine-readable XML (`tools/dev/isa_table_check.py`) | xshader, d3d12_xs |
| [drycheetah84/TomBox PR #2](https://github.com/drycheetah84/TomBox/pull/2) | as TomBox | rtcap NaN/Inf scan over the full resolution with first coordinates (bloom artifact diagnostics) | d3d12_xs rtcap |
| [microsoft/libHttpClient `Source/Task`](https://github.com/microsoft/libHttpClient/tree/main/Source/Task) | MIT | Microsoft's own XTaskQueue/XAsync (ports, monitors, delayed callbacks, cancel, terminate); reference to diff `compat-common/src/gdk/rt.rs` against, e.g. for Terraria idling in `XTaskQueueDispatch` | GDK runtime |
| [minecraft-linux/xgameruntime-rs](https://github.com/minecraft-linux/xgameruntime-rs) | MIT | Rust XStore / XPackage / XNetworking / XGameStreaming / XPersistentLocalStorage implementations, class and interface IDs (`src/com.rs`), XAsyncBlock internals (`src/threading.rs`) | GDK runtime |
| [GPUOpen-Drivers/pal](https://github.com/GPUOpen-Drivers/pal) | MIT | `PM4_PFP_SET_PREDICATION` (gfx9_plus_merged_f32_pfp_pm4_packets.h) and `CmdUtil::BuildSetPredication`: pred_op 3 = DX12 bool64 (8-byte aligned), continue bit only legal for ZPASS/PRIMCOUNT. Consistent with the +8 step assumed for AdvancePredicationX (issue #10) but does not prove it | d3d12_xs predication |
| [Mesa](https://gitlab.freedesktop.org/mesa/mesa) `src/amd` | MIT | NGG prim export / GS_ALLOC_REQ packing and GFX10 fully-culled workaround (`ac_nir_prerast_utils.c`), NGG SGPR fields (`ac_shader_args.h`), task/mesh ring protocol (`ac_nir_lower_taskmesh_io_to_mem.c`) for Mesh/Amplification stages; ZPASS per-RB result layout (`radeonsi/si_query.c`) and DB_COUNT_CONTROL setup (`radv_cmd_buffer.c`) for occlusion queries; ACO `ds_ordered_count` offset bits | xshader stages, query_x |
| [LLVM AMDGPU](https://github.com/llvm/llvm-project/tree/main/llvm/lib/Target/AMDGPU) | Apache-2.0 WITH LLVM-exception | GFX10 `v_cmpx` writes EXEC only (FeatureNoSdstCMPX); byte-exact encoding vectors in `llvm/test/MC/AMDGPU/gfx10_asm_{vopc,vopcx,ds}.s`; GWS resource id `(base + M0[21:16] + offset) % 64` | xshader |
| [XWine1/SlimEra](https://github.com/XWine1/SlimEra) | MIT | ERA PIX (`PIXCaptureParameters` with Xbox-only fields), Etx*, toolhelpx signatures | pix*, etw |
| [Ender-0001/GDKScarlett](https://github.com/Ender-0001/GDKScarlett) | BSD-2-Clause | Xbox One d3d12_x signatures for ScheduleFrameEventX / WaitFrameEventX / GetFrameStatisticsX (bodies are stubs) | d3d12_x |

## Reference only (licence)

| Project | Licence | What it shows |
|---|---|---|
| [WinDurango/WinDurango](https://github.com/WinDurango/WinDurango) | LGPL-3.0 | ERA ACP/Logan/SHAPE: ACP command numbers (EQCOMP 0x0a, FILTVOL 0x0b, PCM 0x0d), SHAPE flowgraph command bit layouts (`ACPLoganTypes.h`), XMA via FFmpeg with priming dedupe. Xbox One ABI; Scarlett acphal may differ |
| [Freminet/ScarlettX](https://github.com/Freminet/ScarlettX) | none | Claims a Unity IL2CPP title in-game with audio/input/saves while its RDNA2 lifting is 0%, i.e. rendering from DXIL kept in the shader containers (inference). A Unity title may be the cheapest second title |
| [xodus-gaming/xgameruntime](https://github.com/xodus-gaming/xgameruntime) | LGPL-2.1 | IDL method order for XGameUI, XUser, XPackage, XStore, XSystem, XAppCapture, XDisplay |
| [shadps4-emu/shadPS4](https://github.com/shadps4-emu/shadPS4) | GPL-2.0 | GCN DS/GDS lowering; ds_append/consume take the GDS base from M0[31:16]. Its ds_inc/dec ignore DATA (wrong per ISA) |
| [mattias800/prosper](https://github.com/mattias800/prosper) | none | RDNA2 NGG run as a compute "subgroup shell" plus pass-through draw |
| [LooperSalty/AnyXboxX](https://github.com/LooperSalty/AnyXboxX) | none | GDK interface/method numbers extracted from `xgameruntime.lib` (`tools/gdk_interfaces.py`) |
| [Coolaid003/XboxSeries](https://github.com/Coolaid003/XboxSeries) | none | Dev-mode notes: xbtp.dll and the Xtf* DLLs belong to the developer tool chain (issues #1-#3) |

## Not useful

No code or unrelated: SerfOscillator/gta-6-early-access (off-site download link only, avoid), kalkidanh/XbloX (UI mock-up), hue-jaw/xbox-one-emulator-windows, Marco-Dev120/XBOX-ONE-EMULATION, RiverDev0/XB1 (empty), californiaAlmonds/OpenScarlett (its "XMA2" decoder is an invented codec), jadeowaowa/scarlett-layer (self-described generated, modelled vtables). No public project implements AdvancePredicationX, Fill64 ordering, frame pacing, xbtp sessions, pixrt10 `PixDllInitialize`, or the other console-library gaps in issues #1-#9.
