// Synthetic x64 title for the d3d12_xs compatibility DLL. Build and launch with
// run_d3d12_guest.py from an x64 Visual Studio developer shell.
#include <windows.h>
#include <d3d12.h>
#include <dxgi.h>
#include <wrl/client.h>

#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>

using Microsoft::WRL::ComPtr;

extern "C" __declspec(dllimport) HRESULT WINAPI D3D12XboxCreateDevice(
    IUnknown*, const void*, REFIID, void**);

// device_wrap.rs, heap_wrap.rs and cmdlist_wrap.rs describe these Scarlett
// methods. Use SDK interfaces directly for slots with the same signature.
template <typename Method>
static Method xbox_method(IUnknown* object, size_t slot) {
    return reinterpret_cast<Method>((*reinterpret_cast<void***>(object))[slot]);
}

using CreateListX = HRESULT (WINAPI*)(ID3D12Device*, const UINT*, ID3D12CommandAllocator*,
                                     ID3D12PipelineState*, REFIID, void**);
using GetCpuHandleX = SIZE_T (WINAPI*)(ID3D12DescriptorHeap*);
using ClearRtvX = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE,
                                const float*, UINT, const D3D12_RECT*);
using CopyTextureX = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                                   UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);

static void check(HRESULT hr, const char* operation) {
    if (FAILED(hr)) {
        char message[160];
        sprintf_s(message, "d3d12 guest %s failed (HRESULT 0x%08lx)", operation, static_cast<unsigned long>(hr));
        OutputDebugStringA(message);
        std::exit(1);
    }
}

static void report_adapter(ID3D12Device* device) {
    ComPtr<IDXGIDevice> dxgi;
    check(device->QueryInterface(IID_PPV_ARGS(&dxgi)), "QueryInterface(IDXGIDevice)");
    ComPtr<IDXGIAdapter> adapter;
    check(dxgi->GetAdapter(&adapter), "GetAdapter");
    DXGI_ADAPTER_DESC desc{};
    check(adapter->GetDesc(&desc), "GetDesc");
    char name[512], message[580];
    if (!WideCharToMultiByte(CP_UTF8, 0, desc.Description, -1, name, sizeof(name), nullptr, nullptr)) {
        check(HRESULT_FROM_WIN32(GetLastError()), "adapter description");
    }
    sprintf_s(message, "d3d12 guest pid=%lu adapter=%s (%04x:%04x)", GetCurrentProcessId(),
              name, desc.VendorId, desc.DeviceId);
    OutputDebugStringA(message);
}

// CreateCommittedResource in device_wrap.rs accepts a 0x40-byte console descriptor.
struct XboxResourceDesc {
    D3D12_RESOURCE_DESC desc{};
    UINT64 reserved{};
};
static_assert(sizeof(XboxResourceDesc) == 0x40);

int main(int argc, char** argv) {
    if (argc != 2 || (strcmp(argv[1], "red") && strcmp(argv[1], "green"))) {
        OutputDebugStringA("usage: d3d12_guest.exe red|green");
        return 2;
    }
    const bool red = strcmp(argv[1], "red") == 0;
    const float color[] = {red ? 1.f : 0.f, red ? 0.f : 1.f, 0.f, 1.f};

    ComPtr<ID3D12Device> device;
    // Version from tests/common_process_debug.rs, with zero flags for the stock driver.
    const UINT create_params[32] = {0x00020002};
    check(D3D12XboxCreateDevice(nullptr, create_params, IID_PPV_ARGS(&device)), "D3D12XboxCreateDevice");
    report_adapter(device.Get());
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "CreateCommandQueue");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "CreateCommandAllocator");
    const UINT list_params[] = {D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    auto create_list = xbox_method<CreateListX>(device.Get(), 73);
    check(create_list(device.Get(), list_params, allocator.Get(), nullptr, IID_PPV_ARGS(&list)), "CreateCommandListX");

    constexpr UINT width = 4, height = 4;
    constexpr DXGI_FORMAT format = DXGI_FORMAT_R8G8B8A8_UNORM;
    XboxResourceDesc target_desc{};
    target_desc.desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    target_desc.desc.Width = width;
    target_desc.desc.Height = height;
    target_desc.desc.DepthOrArraySize = 1;
    target_desc.desc.MipLevels = 1;
    target_desc.desc.Format = format;
    target_desc.desc.SampleDesc.Count = 1;
    target_desc.desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    ComPtr<ID3D12Resource> target;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &target_desc.desc,
          D3D12_RESOURCE_STATE_RENDER_TARGET, nullptr, IID_PPV_ARGS(&target)), "create render target");

    D3D12_DESCRIPTOR_HEAP_DESC rtv_desc{};
    rtv_desc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV;
    rtv_desc.NumDescriptors = 1;
    ComPtr<ID3D12DescriptorHeap> rtv_heap;
    check(device->CreateDescriptorHeap(&rtv_desc, IID_PPV_ARGS(&rtv_heap)), "CreateDescriptorHeap");
    D3D12_CPU_DESCRIPTOR_HANDLE rtv{xbox_method<GetCpuHandleX>(rtv_heap.Get(), 9)(rtv_heap.Get())};
    device->CreateRenderTargetView(target.Get(), nullptr, rtv);

    constexpr UINT pitch = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
    static_assert(width * 4 <= pitch);
    XboxResourceDesc buffer_desc{};
    buffer_desc.desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    buffer_desc.desc.Width = pitch * height;
    buffer_desc.desc.Height = 1;
    buffer_desc.desc.DepthOrArraySize = 1;
    buffer_desc.desc.MipLevels = 1;
    buffer_desc.desc.SampleDesc.Count = 1;
    buffer_desc.desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    heap.Type = D3D12_HEAP_TYPE_READBACK;
    ComPtr<ID3D12Resource> readback;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc.desc,
          D3D12_RESOURCE_STATE_COPY_DEST, nullptr, IID_PPV_ARGS(&readback)), "create readback buffer");

    xbox_method<ClearRtvX>(list.Get(), 52)(list.Get(), rtv, color, 0, nullptr);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition = {target.Get(), 0, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = target.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    dst.pResource = readback.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint.Footprint = {format, width, height, 1, pitch};
    xbox_method<CopyTextureX>(list.Get(), 83)(list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
    check(list->Close(), "Close");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "CreateFence");
    ID3D12CommandList* lists[] = {list.Get()};
    queue->ExecuteCommandLists(1, lists);
    check(queue->Signal(fence.Get(), 1), "Signal");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) {
        check(HRESULT_FROM_WIN32(GetLastError()), "CreateEvent");
    }
    check(fence->SetEventOnCompletion(1, event), "SetEventOnCompletion");
    DWORD wait = WaitForSingleObject(event, 10000);
    CloseHandle(event);
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1) {
        OutputDebugStringA("d3d12 guest fence did not complete at 1");
        return 1;
    }

    constexpr UINT x = 2, y = 1;
    constexpr SIZE_T offset = y * pitch + x * 4;
    const D3D12_RANGE range{offset, offset + 4}, no_writes{0, 0};
    void* mapped = nullptr;
    check(readback->Map(0, &range, &mapped), "Map");
    std::array<BYTE, 4> pixel;
    memcpy(pixel.data(), static_cast<const BYTE*>(mapped) + offset, pixel.size());
    readback->Unmap(0, &no_writes);
    char message[120];
    sprintf_s(message, "d3d12 guest %s pixel(%u,%u)=%02x %02x %02x %02x",
              argv[1], x, y, pixel[0], pixel[1], pixel[2], pixel[3]);
    OutputDebugStringA(message);
    const std::array<BYTE, 4> expected_red{255, 0, 0, 255}, expected_green{0, 255, 0, 255};
    return pixel == (red ? expected_red : expected_green) ? 0 : 1;
}
