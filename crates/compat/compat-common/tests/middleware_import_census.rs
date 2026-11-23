use compat_common::os_host::routes::{self, lookup};
use core::ffi::c_void;
use std::path::{Path, PathBuf};

#[path = "support/built_dlls.rs"]
mod built_dlls;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
}

const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;

const BUILDS: &[u32] = &[19041, 22000, 22621];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum S {
    Served,
    NotServed,
}

#[allow(unused_imports)]
use S::{NotServed as N, Served as Y};

const CONSOLE: &[(&str, &str, &str, S)] = &[
    ("amd_fidelityfx_upscaler_xbox.dll", "d3d12_xs.dll", "D3D12CreateRootSignatureDeserializer", Y),
    ("amd_fidelityfx_upscaler_xbox.dll", "xmem.dll", "XMemAlloc", Y),
    ("amd_fidelityfx_upscaler_xbox.dll", "xmem.dll", "XMemFree", Y),
    ("amd_fidelityfx_upscaler_xbox.dll", "xmem.dll", "XMemGetWorkingSetStatistics", Y),
    ("Bink2Scarlett.dll", "api-ms-win-core-processtopology-obsolete-l1-1-0.dll", "SetThreadAffinityMask", Y),
    ("Bink2Scarlett.dll", "XAudio2_9.dll", "#1", Y),
    ("Bink2Scarlett.dll", "xmem.dll", "XMemAlloc", Y),
    ("Bink2Scarlett.dll", "xmem.dll", "XMemFree", Y),
    ("XboxBvhBuilder.dll", "d3d12_xs.dll", "D3D12SerializeVersionedRootSignature", Y),
    ("XboxBvhBuilder.dll", "xmem.dll", "XMemVirtualQuery", Y),
    ("XCurl.dll", "api-ms-win-core-psm-appnotify-l1-1-0.dll", "RegisterAppStateChangeNotification", Y),
    ("XCurl.dll", "api-ms-win-core-psm-appnotify-l1-1-0.dll", "UnregisterAppStateChangeNotification", Y),
    ("XCurl.dll", "api-ms-win-core-registry-l2-1-0.dll", "RegOpenKeyW", Y),
];

const HOST: &[(&str, &str, &str)] = &[
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-interlocked-l1-1-0.dll", "InitializeSListHead"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "DisableThreadLibraryCalls"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "GetModuleHandleA"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "GetProcAddress"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-libraryloader-l1-2-1.dll", "LoadLibraryA"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcessId"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentThreadId"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceCounter"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetSystemTimeAsFileTime"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-heap-l1-1-0.dll", "calloc"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-heap-l1-1-0.dll", "free"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_cexit"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_configure_narrow_argv"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_execute_onexit_table"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_narrow_environment"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm_e"),
    ("amd_fidelityfx_loader_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_seh_filter_dll"),
    ("amd_fidelityfx_loader_xbox.dll", "VCRUNTIME140.dll", "__C_specific_handler"),
    ("amd_fidelityfx_loader_xbox.dll", "VCRUNTIME140.dll", "__std_type_info_destroy_list"),
    ("amd_fidelityfx_loader_xbox.dll", "VCRUNTIME140.dll", "memcpy"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-debug-l1-1-0.dll", "DebugBreak"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-debug-l1-1-0.dll", "IsDebuggerPresent"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-debug-l1-1-0.dll", "OutputDebugStringA"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-debug-l1-1-0.dll", "OutputDebugStringW"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-interlocked-l1-1-0.dll", "InitializeSListHead"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "DisableThreadLibraryCalls"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-localization-l1-2-0.dll", "FormatMessageW"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-processenvironment-l1-1-0.dll", "GetEnvironmentVariableA"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-processenvironment-l1-1-0.dll", "SetEnvironmentVariableA"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcessId"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentThreadId"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-processthreads-l1-1-1.dll", "IsProcessorFeaturePresent"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceCounter"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-string-l1-1-0.dll", "MultiByteToWideChar"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-string-l1-1-0.dll", "WideCharToMultiByte"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-synch-l1-1-0.dll", "AcquireSRWLockExclusive"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-synch-l1-1-0.dll", "ReleaseSRWLockExclusive"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-synch-l1-2-0.dll", "SleepConditionVariableSRW"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-synch-l1-2-0.dll", "WakeAllConditionVariable"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetSystemTimeAsFileTime"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-convert-l1-1-0.dll", "wcstombs_s"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-environment-l1-1-0.dll", "_putenv_s"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-heap-l1-1-0.dll", "_callnewh"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-heap-l1-1-0.dll", "calloc"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-heap-l1-1-0.dll", "free"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-heap-l1-1-0.dll", "malloc"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-math-l1-1-0.dll", "_dsign"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-math-l1-1-0.dll", "_fdsign"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-math-l1-1-0.dll", "_ldsign"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-math-l1-1-0.dll", "exp2f"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-math-l1-1-0.dll", "log2"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-math-l1-1-0.dll", "log2f"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_cexit"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_configure_narrow_argv"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_crt_atexit"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_execute_onexit_table"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_narrow_environment"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm_e"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_register_onexit_function"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_seh_filter_dll"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf_s"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vswprintf_s"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-string-l1-1-0.dll", "strlen"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcscmp"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcscpy_s"),
    ("amd_fidelityfx_upscaler_xbox.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcslen"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "??0_Locinfo@std@@QEAA@PEBD@Z"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "??0_Lockit@std@@QEAA@H@Z"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "??0facet@locale@std@@IEAA@_K@Z"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "??1_Locinfo@std@@QEAA@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "??1_Lockit@std@@QEAA@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "??1facet@locale@std@@MEAA@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Decref@facet@locale@std@@UEAAPEAV_Facet_base@3@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Getcvt@_Locinfo@std@@QEBA?AU_Cvtvec@@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Getfalse@_Locinfo@std@@QEBAPEBDXZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Getgloballocale@locale@std@@CAPEAV_Locimp@12@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Getlconv@_Locinfo@std@@QEBAPEBUlconv@@XZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Gettrue@_Locinfo@std@@QEBAPEBDXZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Id_cnt@id@locale@std@@0HA"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Incref@facet@locale@std@@UEAAXXZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Init@locale@std@@CAPEAV_Locimp@12@_N@Z"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Throw_Cpp_error@std@@YAXH@Z"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Xbad_alloc@std@@YAXXZ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?_Xlength_error@std@@YAXPEBD@Z"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?id@?$numpunct@D@std@@2V0locale@2@A"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "?id@?$numpunct@_W@std@@2V0locale@2@A"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "_Mbrtowc"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "_Mtx_init_in_situ"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "_Mtx_lock"),
    ("amd_fidelityfx_upscaler_xbox.dll", "MSVCP140.dll", "_Mtx_unlock"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "_CxxThrowException"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "__C_specific_handler"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "__std_exception_copy"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "__std_exception_destroy"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "__std_terminate"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "__std_type_info_destroy_list"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "memcpy"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "memmove"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140.dll", "memset"),
    ("amd_fidelityfx_upscaler_xbox.dll", "VCRUNTIME140_1.dll", "__CxxFrameHandler4"),
    ("Bink2Scarlett.dll", "api-ms-win-core-com-l1-1-0.dll", "CoCreateInstance"),
    ("Bink2Scarlett.dll", "api-ms-win-core-com-l1-1-0.dll", "CoTaskMemFree"),
    ("Bink2Scarlett.dll", "api-ms-win-core-com-l1-1-0.dll", "PropVariantClear"),
    ("Bink2Scarlett.dll", "api-ms-win-core-debug-l1-1-0.dll", "IsDebuggerPresent"),
    ("Bink2Scarlett.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "GetLastError"),
    ("Bink2Scarlett.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "RaiseException"),
    ("Bink2Scarlett.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "SetUnhandledExceptionFilter"),
    ("Bink2Scarlett.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "UnhandledExceptionFilter"),
    ("Bink2Scarlett.dll", "api-ms-win-core-errorhandling-l1-1-1.dll", "AddVectoredExceptionHandler"),
    ("Bink2Scarlett.dll", "api-ms-win-core-errorhandling-l1-1-1.dll", "RemoveVectoredExceptionHandler"),
    ("Bink2Scarlett.dll", "api-ms-win-core-file-l1-1-0.dll", "GetFileSizeEx"),
    ("Bink2Scarlett.dll", "api-ms-win-core-file-l1-1-0.dll", "ReadFile"),
    ("Bink2Scarlett.dll", "api-ms-win-core-file-l1-1-0.dll", "SetFilePointerEx"),
    ("Bink2Scarlett.dll", "api-ms-win-core-file-l1-2-0.dll", "CreateFile2"),
    ("Bink2Scarlett.dll", "api-ms-win-core-handle-l1-1-0.dll", "CloseHandle"),
    ("Bink2Scarlett.dll", "api-ms-win-core-interlocked-l1-1-0.dll", "InitializeSListHead"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "CreateThread"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcess"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcessId"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentThread"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentThreadId"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetThreadPriority"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "ResumeThread"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "SetThreadPriority"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "TerminateProcess"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-1.dll", "IsProcessorFeaturePresent"),
    ("Bink2Scarlett.dll", "api-ms-win-core-processthreads-l1-1-1.dll", "SetThreadIdealProcessorEx"),
    ("Bink2Scarlett.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceCounter"),
    ("Bink2Scarlett.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceFrequency"),
    ("Bink2Scarlett.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlCaptureContext"),
    ("Bink2Scarlett.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlLookupFunctionEntry"),
    ("Bink2Scarlett.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlVirtualUnwind"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-1-0.dll", "DeleteCriticalSection"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-1-0.dll", "EnterCriticalSection"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-1-0.dll", "InitializeCriticalSection"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-1-0.dll", "LeaveCriticalSection"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-1-0.dll", "WaitForSingleObject"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-2-0.dll", "InitializeConditionVariable"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-2-0.dll", "Sleep"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-2-0.dll", "SleepConditionVariableCS"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-2-0.dll", "WakeAllConditionVariable"),
    ("Bink2Scarlett.dll", "api-ms-win-core-synch-l1-2-0.dll", "WakeConditionVariable"),
    ("Bink2Scarlett.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetSystemTimeAsFileTime"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_cexit"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_configure_narrow_argv"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_execute_onexit_table"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_narrow_environment"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm_e"),
    ("Bink2Scarlett.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_seh_filter_dll"),
    ("Bink2Scarlett.dll", "VCRUNTIME140.dll", "__C_specific_handler"),
    ("Bink2Scarlett.dll", "VCRUNTIME140.dll", "__std_type_info_destroy_list"),
    ("Bink2Scarlett.dll", "VCRUNTIME140.dll", "memcpy"),
    ("Bink2Scarlett.dll", "VCRUNTIME140.dll", "memmove"),
    ("Bink2Scarlett.dll", "VCRUNTIME140.dll", "memset"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "EventRegister"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "EventSetInformation"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "EventUnregister"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "EventWriteTransfer"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "RegCloseKey"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "RegOpenKeyExW"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "RegOpenKeyW"),
    ("libHttpClient.GDK.dll", "ADVAPI32.dll", "RegQueryValueExW"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-convert-l1-1-0.dll", "_wtoi"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-convert-l1-1-0.dll", "strtoull"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-convert-l1-1-0.dll", "wcstombs"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-convert-l1-1-0.dll", "wcstoull"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-heap-l1-1-0.dll", "_callnewh"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-heap-l1-1-0.dll", "calloc"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-heap-l1-1-0.dll", "free"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-heap-l1-1-0.dll", "malloc"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-math-l1-1-0.dll", "ceilf"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-math-l1-1-0.dll", "pow"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_cexit"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_configure_narrow_argv"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_crt_at_quick_exit"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_crt_atexit"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_errno"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_execute_onexit_table"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_narrow_environment"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm_e"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_invalid_parameter_noinfo"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_invoke_watson"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_register_onexit_function"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_seh_filter_dll"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_set_errno"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_wassert"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "strerror"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "terminate"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsnprintf_s"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf_s"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_close"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_fseeki64"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_ftelli64"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_lseeki64"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_open"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_read"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_wopen"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "_write"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "fclose"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "ferror"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "fopen"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "fread"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "fseek"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "ftell"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "fwrite"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "_stricmp"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "isspace"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "strcmp"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "strlen"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcscat_s"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcslen"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcstok_s"),
    ("libHttpClient.GDK.dll", "api-ms-win-crt-time-l1-1-0.dll", "_localtime64_s"),
    ("libHttpClient.GDK.dll", "CRYPT32.dll", "CertCloseStore"),
    ("libHttpClient.GDK.dll", "CRYPT32.dll", "CertFindChainInStore"),
    ("libHttpClient.GDK.dll", "CRYPT32.dll", "CertFreeCertificateChain"),
    ("libHttpClient.GDK.dll", "CRYPT32.dll", "CertOpenSystemStoreW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "AcquireSRWLockExclusive"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CallbackMayRunLong"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CloseHandle"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CloseThreadpoolTimer"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CloseThreadpoolWork"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CreateEventW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CreateFileA"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CreateFileW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CreateThreadpoolTimer"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "CreateThreadpoolWork"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "DeleteCriticalSection"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "DisableThreadLibraryCalls"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "DisassociateCurrentThreadFromCallback"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "EnterCriticalSection"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "FreeLibrary"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetCurrentProcess"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetCurrentProcessId"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetCurrentThreadId"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetFileAttributesW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetLastError"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetModuleFileNameW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetModuleHandleW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetProcAddress"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetStartupInfoW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GetSystemTimeAsFileTime"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "GlobalFree"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "InitializeCriticalSection"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "InitializeSListHead"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "IsDebuggerPresent"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "IsProcessorFeaturePresent"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "LeaveCriticalSection"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "LoadLibraryExA"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "LoadLibraryExW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "LoadLibraryW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "MultiByteToWideChar"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "OutputDebugStringA"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "QueryPerformanceCounter"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "ReadFile"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "ReleaseSRWLockExclusive"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "RtlCaptureContext"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "RtlLookupFunctionEntry"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "RtlVirtualUnwind"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "SetEvent"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "SetFilePointer"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "SetThreadpoolTimer"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "SetUnhandledExceptionFilter"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "SleepConditionVariableSRW"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "SubmitThreadpoolWork"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "TerminateProcess"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "UnhandledExceptionFilter"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "WaitForSingleObject"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "WaitForThreadpoolTimerCallbacks"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "WaitForThreadpoolWorkCallbacks"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "WakeAllConditionVariable"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "WideCharToMultiByte"),
    ("libHttpClient.GDK.dll", "KERNEL32.dll", "WriteFile"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??0?$basic_ios@DU?$char_traits@D@std@@@std@@IEAA@XZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??0?$basic_iostream@DU?$char_traits@D@std@@@std@@QEAA@PEAV?$basic_streambuf@DU?$char_traits@D@std@@@1@@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??0?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAA@XZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??1?$basic_ios@DU?$char_traits@D@std@@@std@@UEAA@XZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??1?$basic_iostream@DU?$char_traits@D@std@@@std@@UEAA@XZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??1?$basic_streambuf@DU?$char_traits@D@std@@@std@@UEAA@XZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "??5?$basic_istream@DU?$char_traits@D@std@@@std@@QEAAAEAV01@AEAH@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Getcvt@_Locinfo@std@@QEBA?AU_Cvtvec@@XZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Lock@?$basic_streambuf@DU?$char_traits@D@std@@@std@@UEAAXXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Pninc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAPEADXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Syserror_map@std@@YAPEBDH@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Throw_Cpp_error@std@@YAXH@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Unlock@?$basic_streambuf@DU?$char_traits@D@std@@@std@@UEAAXXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_W_Getdays@_Locinfo@std@@QEBAPEBGXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_W_Getmonths@_Locinfo@std@@QEBAPEBGXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Xbad_alloc@std@@YAXXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Xbad_function_call@std@@YAXXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Xlength_error@std@@YAXPEBD@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?_Xout_of_range@std@@YAXPEBD@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?eback@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?egptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?epptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?fail@ios_base@std@@QEBA_NXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?gbump@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXH@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?gptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?imbue@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAXAEBVlocale@2@@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?pptr@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEBAPEADXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?setbuf@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAPEAV12@PEAD_J@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?setg@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXPEAD00@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?setp@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXPEAD00@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?setp@?$basic_streambuf@DU?$char_traits@D@std@@@std@@IEAAXPEAD0@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?showmanyc@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA_JXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?sync@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?uflow@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAAHXZ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?xsgetn@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA_JPEAD_J@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "?xsputn@?$basic_streambuf@DU?$char_traits@D@std@@@std@@MEAA_JPEBD_J@Z"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "_Mtx_init_in_situ"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "_Mtx_lock"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "_Mtx_unlock"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "_Query_perf_counter"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "_Query_perf_frequency"),
    ("libHttpClient.GDK.dll", "MSVCP140.dll", "_Thrd_yield"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "_CxxThrowException"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__C_specific_handler"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__CxxFrameHandler3"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__RTDynamicCast"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__current_exception"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__current_exception_context"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__std_exception_copy"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__std_exception_destroy"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__std_terminate"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__std_type_info_compare"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "__std_type_info_destroy_list"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "_purecall"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "memchr"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "memcmp"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "memcpy"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "memmove"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "memset"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140.dll", "wcsrchr"),
    ("libHttpClient.GDK.dll", "VCRUNTIME140_1.dll", "__CxxFrameHandler4"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpAddRequestHeaders"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpCloseHandle"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpConnect"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpOpen"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpOpenRequest"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpQueryDataAvailable"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpQueryHeaders"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpQueryOption"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpReadData"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpReceiveResponse"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpSendRequest"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpSetOption"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpSetStatusCallback"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpSetTimeouts"),
    ("libHttpClient.GDK.dll", "WINHTTP.dll", "WinHttpWriteData"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-debug-l1-1-0.dll", "IsDebuggerPresent"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-debug-l1-1-0.dll", "OutputDebugStringA"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-debug-l1-1-0.dll", "OutputDebugStringW"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "SetUnhandledExceptionFilter"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "UnhandledExceptionFilter"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-interlocked-l1-1-0.dll", "InitializeSListHead"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "DisableThreadLibraryCalls"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcess"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcessId"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentThreadId"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "TerminateProcess"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-processthreads-l1-1-1.dll", "IsProcessorFeaturePresent"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceCounter"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlCaptureContext"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlLookupFunctionEntry"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlVirtualUnwind"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-synch-l1-1-0.dll", "AcquireSRWLockExclusive"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-synch-l1-1-0.dll", "ReleaseSRWLockExclusive"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-synch-l1-2-0.dll", "SleepConditionVariableSRW"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-synch-l1-2-0.dll", "WakeAllConditionVariable"),
    ("XboxBvhBuilder.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetSystemTimeAsFileTime"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-heap-l1-1-0.dll", "_callnewh"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-heap-l1-1-0.dll", "free"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-heap-l1-1-0.dll", "malloc"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-math-l1-1-0.dll", "_isnan"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_cexit"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_configure_narrow_argv"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_errno"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_execute_onexit_table"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_narrow_environment"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm_e"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_invalid_parameter_noinfo"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_invalid_parameter_noinfo_noreturn"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_seh_filter_dll"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "terminate"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf_s"),
    ("XboxBvhBuilder.dll", "api-ms-win-crt-string-l1-1-0.dll", "strncpy_s"),
    ("XboxBvhBuilder.dll", "MSVCP140.dll", "?_Xbad_function_call@std@@YAXXZ"),
    ("XboxBvhBuilder.dll", "MSVCP140.dll", "?_Xlength_error@std@@YAXPEBD@Z"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "_CxxThrowException"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__C_specific_handler"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__current_exception"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__current_exception_context"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__std_exception_copy"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__std_exception_destroy"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__std_terminate"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "__std_type_info_destroy_list"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "memcmp"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "memcpy"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "memmove"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140.dll", "memset"),
    ("XboxBvhBuilder.dll", "VCRUNTIME140_1.dll", "__CxxFrameHandler4"),
    ("XCurl.dll", "api-ms-win-core-com-l1-1-0.dll", "CoCreateGuid"),
    ("XCurl.dll", "api-ms-win-core-debug-l1-1-0.dll", "OutputDebugStringA"),
    ("XCurl.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "GetLastError"),
    ("XCurl.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "RaiseException"),
    ("XCurl.dll", "api-ms-win-core-errorhandling-l1-1-0.dll", "SetLastError"),
    ("XCurl.dll", "api-ms-win-core-errorhandling-l1-1-2.dll", "RaiseFailFastException"),
    ("XCurl.dll", "api-ms-win-core-fibers-l1-1-0.dll", "FlsAlloc"),
    ("XCurl.dll", "api-ms-win-core-fibers-l1-1-0.dll", "FlsFree"),
    ("XCurl.dll", "api-ms-win-core-fibers-l1-1-0.dll", "FlsGetValue"),
    ("XCurl.dll", "api-ms-win-core-fibers-l1-1-0.dll", "FlsSetValue"),
    ("XCurl.dll", "api-ms-win-core-file-l1-1-0.dll", "GetFileAttributesW"),
    ("XCurl.dll", "api-ms-win-core-handle-l1-1-0.dll", "CloseHandle"),
    ("XCurl.dll", "api-ms-win-core-heap-l1-1-0.dll", "GetProcessHeap"),
    ("XCurl.dll", "api-ms-win-core-heap-l1-1-0.dll", "HeapAlloc"),
    ("XCurl.dll", "api-ms-win-core-heap-l1-1-0.dll", "HeapFree"),
    ("XCurl.dll", "api-ms-win-core-heap-l1-1-0.dll", "HeapReAlloc"),
    ("XCurl.dll", "api-ms-win-core-interlocked-l1-1-0.dll", "InitializeSListHead"),
    ("XCurl.dll", "api-ms-win-core-interlocked-l1-1-0.dll", "InterlockedFlushSList"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "DisableThreadLibraryCalls"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "FreeLibrary"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "GetModuleFileNameW"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "GetProcAddress"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "LoadLibraryExA"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-0.dll", "LoadLibraryExW"),
    ("XCurl.dll", "api-ms-win-core-libraryloader-l1-2-1.dll", "LoadLibraryW"),
    ("XCurl.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentProcessId"),
    ("XCurl.dll", "api-ms-win-core-processthreads-l1-1-0.dll", "GetCurrentThreadId"),
    ("XCurl.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceCounter"),
    ("XCurl.dll", "api-ms-win-core-profile-l1-1-0.dll", "QueryPerformanceFrequency"),
    ("XCurl.dll", "api-ms-win-core-registry-l1-1-0.dll", "RegCloseKey"),
    ("XCurl.dll", "api-ms-win-core-registry-l1-1-0.dll", "RegOpenKeyExW"),
    ("XCurl.dll", "api-ms-win-core-registry-l1-1-0.dll", "RegQueryValueExW"),
    ("XCurl.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlLookupFunctionEntry"),
    ("XCurl.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlPcToFileHeader"),
    ("XCurl.dll", "api-ms-win-core-rtlsupport-l1-1-0.dll", "RtlUnwindEx"),
    ("XCurl.dll", "api-ms-win-core-string-l1-1-0.dll", "MultiByteToWideChar"),
    ("XCurl.dll", "api-ms-win-core-string-l1-1-0.dll", "WideCharToMultiByte"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "AcquireSRWLockExclusive"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "AcquireSRWLockShared"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "CreateEventW"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "CreateMutexW"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "DeleteCriticalSection"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "EnterCriticalSection"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "InitializeCriticalSectionEx"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "LeaveCriticalSection"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "ReleaseMutex"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "ReleaseSRWLockExclusive"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "ReleaseSRWLockShared"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "ResetEvent"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "SetEvent"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-1-0.dll", "WaitForSingleObject"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-2-0.dll", "Sleep"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-2-0.dll", "SleepConditionVariableSRW"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-2-0.dll", "WakeAllConditionVariable"),
    ("XCurl.dll", "api-ms-win-core-synch-l1-2-1.dll", "WaitForMultipleObjects"),
    ("XCurl.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetSystemTimeAsFileTime"),
    ("XCurl.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetTickCount"),
    ("XCurl.dll", "api-ms-win-core-sysinfo-l1-1-0.dll", "GetTickCount64"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "CallbackMayRunLong"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "CloseThreadpoolTimer"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "CloseThreadpoolWait"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "CloseThreadpoolWork"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "CreateThreadpoolTimer"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "CreateThreadpoolWait"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "DisassociateCurrentThreadFromCallback"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "SetThreadpoolTimer"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "SetThreadpoolWait"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "SubmitThreadpoolWork"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "WaitForThreadpoolWaitCallbacks"),
    ("XCurl.dll", "api-ms-win-core-threadpool-l1-2-0.dll", "WaitForThreadpoolWorkCallbacks"),
    ("XCurl.dll", "api-ms-win-core-util-l1-1-0.dll", "EncodePointer"),
    ("XCurl.dll", "api-ms-win-crt-convert-l1-1-0.dll", "atoi"),
    ("XCurl.dll", "api-ms-win-crt-convert-l1-1-0.dll", "strtol"),
    ("XCurl.dll", "api-ms-win-crt-convert-l1-1-0.dll", "strtoul"),
    ("XCurl.dll", "api-ms-win-crt-heap-l1-1-0.dll", "_calloc_base"),
    ("XCurl.dll", "api-ms-win-crt-heap-l1-1-0.dll", "_free_base"),
    ("XCurl.dll", "api-ms-win-crt-heap-l1-1-0.dll", "free"),
    ("XCurl.dll", "api-ms-win-crt-heap-l1-1-0.dll", "malloc"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_cexit"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_configure_narrow_argv"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_crt_atexit"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_errno"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_execute_onexit_table"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_narrow_environment"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_initterm_e"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_invalid_parameter_noinfo"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_invalid_parameter_noinfo_noreturn"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_register_onexit_function"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "_seh_filter_dll"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "abort"),
    ("XCurl.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "terminate"),
    ("XCurl.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf"),
    ("XCurl.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsprintf_s"),
    ("XCurl.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vsscanf"),
    ("XCurl.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "__stdio_common_vswprintf"),
    ("XCurl.dll", "api-ms-win-crt-stdio-l1-1-0.dll", "fwrite"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "_stricmp"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "strcmp"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "strcpy_s"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "strcspn"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "strncmp"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "strspn"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "tolower"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcscat_s"),
    ("XCurl.dll", "api-ms-win-crt-string-l1-1-0.dll", "wcsncmp"),
    ("XCurl.dll", "api-ms-win-eventing-classicprovider-l1-1-0.dll", "GetTraceEnableFlags"),
    ("XCurl.dll", "api-ms-win-eventing-classicprovider-l1-1-0.dll", "GetTraceEnableLevel"),
    ("XCurl.dll", "api-ms-win-eventing-classicprovider-l1-1-0.dll", "GetTraceLoggerHandle"),
    ("XCurl.dll", "api-ms-win-eventing-classicprovider-l1-1-0.dll", "RegisterTraceGuidsW"),
    ("XCurl.dll", "api-ms-win-eventing-classicprovider-l1-1-0.dll", "TraceMessage"),
    ("XCurl.dll", "api-ms-win-eventing-classicprovider-l1-1-0.dll", "UnregisterTraceGuids"),
    ("XCurl.dll", "api-ms-win-eventing-provider-l1-1-0.dll", "EventRegister"),
    ("XCurl.dll", "api-ms-win-eventing-provider-l1-1-0.dll", "EventSetInformation"),
    ("XCurl.dll", "api-ms-win-eventing-provider-l1-1-0.dll", "EventUnregister"),
    ("XCurl.dll", "api-ms-win-eventing-provider-l1-1-0.dll", "EventWriteTransfer"),
    ("XCurl.dll", "CRYPT32.dll", "CertCloseStore"),
    ("XCurl.dll", "CRYPT32.dll", "CertFindCertificateInStore"),
    ("XCurl.dll", "CRYPT32.dll", "CertFreeCertificateContext"),
    ("XCurl.dll", "CRYPT32.dll", "CertOpenStore"),
    ("XCurl.dll", "CRYPT32.dll", "CryptStringToBinaryW"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpAddRequestHeaders"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpCloseHandle"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpConnect"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpCrackUrl"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpOpen"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpOpenRequest"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpQueryDataAvailable"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpQueryHeaders"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpReadData"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpReceiveResponse"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpSendRequest"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpSetOption"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpSetStatusCallback"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpSetTimeouts"),
    ("XCurl.dll", "WINHTTP.dll", "WinHttpWriteData"),
];

const UNRESOLVED_ON_HOST: &[(&str, &str, &str)] = &[
];

fn rd16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as usize
}

fn rd32(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize
}

fn exports(path: &Path) -> (Vec<String>, Vec<u16>) {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let pe = rd32(&b, 0x3c);
    assert_eq!(&b[pe..pe + 4], b"PE\0\0", "{}", path.display());
    let nsec = rd16(&b, pe + 6);
    let opt = pe + 24;
    let secs = opt + rd16(&b, pe + 20);
    let off = |rva: usize| -> usize {
        (0..nsec)
            .map(|i| secs + i * 40)
            .find_map(|s| {
                let (vsz, va, raw, ptr) = (rd32(&b, s + 8), rd32(&b, s + 12), rd32(&b, s + 16), rd32(&b, s + 20));
                (va <= rva && rva < va + vsz.max(raw)).then(|| ptr + rva - va)
            })
            .unwrap_or_else(|| panic!("rva {rva:#x} outside {}", path.display()))
    };
    let dir = rd32(&b, opt + 112);
    if dir == 0 {
        return (Vec::new(), Vec::new());
    }
    let e = off(dir);
    let base = rd32(&b, e + 16);
    let (nfun, nnames) = (rd32(&b, e + 20), rd32(&b, e + 24));
    let (funcs, names) = (off(rd32(&b, e + 28)), off(rd32(&b, e + 32)));
    let cstr = |o: usize| String::from_utf8_lossy(&b[o..o + b[o..].iter().position(|c| *c == 0).unwrap()]).into_owned();
    let named = (0..nnames).map(|i| cstr(off(rd32(&b, names + i * 4)))).collect();
    let ords = (0..nfun).filter(|i| rd32(&b, funcs + i * 4) != 0).map(|i| (base + i) as u16).collect();
    (named, ords)
}

fn crate_dll(module: &str) -> Option<(String, String)> {
    let stem = module.to_ascii_lowercase();
    let stem = stem.strip_suffix(".dll").unwrap_or(&stem);
    let toml = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(stem).join("Cargo.toml")).ok()?;
    let mut section = "";
    let (mut package, mut lib, mut cdylib) = (None, None, false);
    for l in toml.lines().map(str::trim) {
        if l.starts_with('[') {
            section = l;
            continue;
        }
        let value = l.split_once('=').map(|(_, v)| v.trim().trim_matches('"').to_string());
        match (section, l.split('=').next().map(str::trim)) {
            ("[package]", Some("name")) => package = value,
            ("[lib]", Some("name")) => lib = value,
            ("[lib]", Some("crate-type")) => cdylib = l.contains("cdylib"),
            _ => {}
        }
    }
    if !cdylib {
        return None;
    }
    let package = package?;
    let file = format!("{}.dll", lib.unwrap_or_else(|| package.replace('-', "_")));
    Some((package, file))
}

fn built(package: &str, file: &str) -> PathBuf {
    built_dlls::ensure(&[(package, file)]).join(file)
}

fn routed(module: &str, func: &str) -> S {
    let hits: Vec<bool> = BUILDS.iter().map(|b| lookup(module, func, *b).is_some()).collect();
    assert!(hits.iter().all(|h| *h == hits[0]), "{module}!{func} is routed for some OS builds only: {BUILDS:?} -> {hits:?}");
    if hits[0] {
        Y
    } else {
        N
    }
}

fn status(module: &str, func: &str) -> S {
    let lower = module.to_ascii_lowercase();
    if routes::is_api_set(&lower) || routes::console_dlls().contains(&lower.as_str()) {
        return routed(module, func);
    }
    let Some((package, file)) = crate_dll(module) else { return N };
    let (names, ords) = exports(&built(&package, &file));
    let found = match func.strip_prefix('#') {
        Some(o) => ords.contains(&o.parse::<u16>().unwrap()),
        None => names.iter().any(|n| n == func),
    };
    if found {
        Y
    } else {
        N
    }
}

fn host_resolves(module: &str, func: &str) -> Result<(), String> {
    let name: Vec<u16> = module.encode_utf16().chain(Some(0)).collect();
    let h = unsafe { LoadLibraryExW(name.as_ptr(), core::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if h.is_null() {
        return Err(format!("{module} does not load from System32"));
    }
    let p = match func.strip_prefix('#') {
        Some(o) => unsafe { GetProcAddress(h, o.parse::<u16>().unwrap() as usize as *const u8) },
        None => {
            let c: Vec<u8> = func.bytes().chain(Some(0)).collect();
            unsafe { GetProcAddress(h, c.as_ptr()) }
        }
    };
    if p.is_null() {
        Err(format!("{module} has no export {func}"))
    } else {
        Ok(())
    }
}

#[test]
fn console_specific_imports_keep_their_served_status() {
    let mut changed = Vec::new();
    for (dll, module, func, want) in CONSOLE {
        let now = status(module, func);
        if now != *want {
            changed.push(format!("{dll} imports {module}!{func}: table says {want:?}, emulator now {now:?}"));
        }
    }
    assert!(changed.is_empty(), "update these rows:\n{}", changed.join("\n"));
}

#[test]
fn every_other_import_resolves_on_the_host_from_system32() {
    let mut broken = Vec::new();
    for (dll, module, func) in HOST {
        if let Err(e) = host_resolves(module, func) {
            broken.push(format!("{dll} imports {module}!{func}: {e}"));
        }
    }
    for (dll, module, func) in UNRESOLVED_ON_HOST {
        if host_resolves(module, func).is_ok() {
            broken.push(format!("{dll} imports {module}!{func}: resolves on this host now, move it to HOST"));
        }
    }
    for line in &broken {
        println!("{line}");
    }
    assert!(broken.is_empty(), "{} host import rows are wrong:\n{}", broken.len(), broken.join("\n"));
}

#[test]
fn rows_are_sorted_unique_and_in_one_table() {
    fn key(r: (&str, &str, &str)) -> (String, String, String) {
        (r.0.to_ascii_lowercase(), r.1.to_ascii_lowercase(), r.2.to_string())
    }
    let console: Vec<_> = CONSOLE.iter().map(|r| key((r.0, r.1, r.2))).collect();
    let host: Vec<_> = HOST.iter().map(|r| key(*r)).collect();
    let unresolved: Vec<_> = UNRESOLVED_ON_HOST.iter().map(|r| key(*r)).collect();
    for (name, t) in [("CONSOLE", &console), ("HOST", &host), ("UNRESOLVED_ON_HOST", &unresolved)] {
        for w in t.windows(2) {
            assert!(w[0] < w[1], "{name}: {:?} then {:?}", w[0], w[1]);
        }
    }
    let mut all: Vec<_> = console.iter().chain(&host).chain(&unresolved).collect();
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "a row is in two tables");
}
