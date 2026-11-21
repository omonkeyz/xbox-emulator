tombox-probe: load and resolve evidence for title-shipped modules on PC

Build: set CARGO_TARGET_DIR=D:\xbox-pc-port\emulator\target-wf-3 and RUSTFLAGS=-C target-feature=+crt-static, then
       cargo build -p tombox-probe --release -j 2   (static CRT so the probe never preloads a host VCRUNTIME140.dll;
       result.preloaded_by_probe_host lists any staged name that was already loaded before the title module)
Run:   powershell -File tools\verify\run_chatprobe.ps1   (writes tools\verify\evidence\*.json; staging goes to
       %TEMP%\tombox-probe-stage, outside the repo; the dumps are only read)
Check: python -I tools\verify\check_coverage.py   (joins progress.json unverified_binaries/unverified_modules with
       title_imports.json and the evidence; writes evidence\coverage.json; exit 1 if any row is missing or unresolved)

Commands (all title-agnostic):
  load  --title T --bin-dir DIR --dll REL --xvd EMBEDDEDXVD --scratch DIR --out F
        [--expect-from REL]... [--search REL]... [--stage FILE]... [--pinvoke-prefix P --pinvoke-from FILE...] [--timeout S]
        Copies REL and its title-shipped dependency closure into a scratch dir (the dump is only read), stages a host
        VC++ 14.x redist DLL when the title does not ship one, stages --stage emulator DLLs, then runs a child process
        that LoadLibraryExW's the copy and checks every row: import rows compare the loader-bound IAT slot against
        LoadLibraryExW+GetProcAddress, delay rows are resolved with ResolveDelayLoadedAPI on the module's own delay
        descriptor, --expect-from rows are GetProcAddress'd on the loaded module, dynamic rows are the
        LoadLibrary/GetProcAddress names recovered from the module's code, pinvoke rows are metadata names.
        When all 12 ConcRT delay imports resolve, the resolved addresses are exercised (critical_section and
        _Condition_variable semantics) and the result is in result.concrt_selftest.
  scan  --title T --bin-dir DIR --xvd EMBEDDEDXVD [--stage-dir DIR]... --out F
        Every import and delay row of every exe/dll under DIR, resolved against the title-shipped module, a staged
        emulator DLL, or the PC System32 module; no title module is loaded.
  token --token NAME --root DIR... --out F      exact module-name search (ASCII and UTF-16, identifier boundaries)
  sites --dll FILE --out F                      dynamic load sites only

Row fields (G9): title, binary, module, function, kind (import|delay|dynamic|pinvoke|delay-of-importer),
resolved, ordinal_meaning, check, pc_host, pc_ordinal, console_host, console_ordinal, console_module_present,
iat_match, null_checked, error, note. Each file has summary {rows, resolved, unresolved, iat_mismatch, failures}.

Findings from the 2026-11-21 run (coverage.json: Fortnite 233/233 unverified rows resolved):
  Fortnite GameChat2.dll 2008.4.0.0 loads on PC; 207/207 imports bind with IAT == GetProcAddress (WS2_32 rows are
  by ordinal: import_ordinal holds it, function holds the console name at that ordinal); the 14 EXE delay imports
  resolve by GetProcAddress (ordinals 1,2,3,4,16,17,18,19,24,31,33,41,42,44).
  AudioHALWrapper.dll has no PC system module. GameChat2 loads it with LoadLibraryExW(L"AudioHALWrapper.dll",0,0)
  at rva 0x47188 (Opus, 16 names) and 0x4a5fc (Silk, 14 names). All 30 names are exported by the console
  AudioHALWrapper.dll, and the console DLL has 2 more that GameChat2 never resolves: SilkGetMaximumDecoderBufferSizes
  and SilkGetMaximumEncoderBufferSizes. With the emulator AudioHALWrapper.dll staged (run_chatprobe.ps1 picks the
  newest target*\release\AudioHALWrapper.dll; the crate belongs to the audio group), all 30 resolve at the console
  ordinals. A NULL module handle becomes HRESULT_FROM_WIN32(GetLastError()); every direct GetProcAddress result is
  NULL-checked; four Silk names go through helper 0x4ac10, whose HRESULT the caller tests with js
  (null_checked=false in the JSON). The failure path clears the function table and returns the HRESULT. The two
  callers (0x456cf in 0x4561c and 0x4644d in 0x4639c) only take this path when a lazily initialised global
  (set by 0x1800531b8) equals 2. On failure they call the object's cleanup routine (0x4576c / 0x464e8), drop the
  object pointer and run a path gated by flag byte +0x94 bit 0. Whether a software codec is used after that is not
  established here.
  Trackmania GameChat2.dll 2210.0.0.0: 204/204 imports bind; same AudioHALWrapper pattern at 0x8cdf8. No Trackmania
  binary (GbxGame.exe, aegir.dll or any other) contains the literal "GameChat2"; only the AppxManifest XML names
  it, so the run-time load name is not a literal and no importer rows exist for it.
  Fortnite MSVCP140.dll 14.24.28315.0: Fortnite ships no CONCRT140.dll (not in Binaries\XSX and not in console
  System32), so the host VC++ redist concrt140.dll is staged next to it. All 12 delay imports resolve through
  ResolveDelayLoadedAPI to the staged copy, with the title's own VCRUNTIME140.dll 14.24 loaded (nothing was
  preloaded), and the ConcRT self-test passes against those resolved addresses. Terraria msvcp140 (Mount) resolves
  its delay imports to its own shipped concrt140.dll 14.28.29325.2. Data\Native\msvcp140 has no ConcRT delay imports.
  xboxlive.dll: no file in Terraria Mount or console System32 contains the token xboxlive.dll (ASCII or UTF-16,
  identifier-bounded). The nearest module names are PartyXboxLive.dll (shipped in Data\Plugins, InternalName
  xboxlive_gamecore) and the IL2CPP P/Invoke module "PartyXboxLive" in GameAssembly/global-metadata. Loading
  PartyXboxLive.dll on PC resolves all 238 imports and every PartyXbl* P/Invoke name from GameAssembly. The only
  unresolved metadata name, PartyXblInterop, is not in GameAssembly's P/Invoke strings (a managed type name).
  WinHTTP websocket entry points are only resolved, never called; the probe makes no network traffic.

XAudio2 PCM guest

From an x64 Visual Studio Developer PowerShell with stable MSVC Rust on PATH, run
  .\tools\verify\run_xaudio_pcm_guest.ps1

The guest imports Xbox XAudio2_9.dll ordinal 1 and submits 20 ms of generated mono PCM at 48 kHz to a
source voice. It uses the default Windows render endpoint with the mastering voice muted. The runner
checks the original buffer context in OnBufferEnd, the OnStreamEnd callback and the child's exit under
mdbg. It prints the shim and backend paths and the directory containing the launch logs.
