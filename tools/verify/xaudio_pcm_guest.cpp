#include <windows.h>
#include <xaudio2.h>
#include <atomic>
#include <cstdio>

// The guest's import library binds this symbol to XAudio2_9.dll ordinal 1.
extern "C" __declspec(dllimport) HRESULT WINAPI GuestXAudio2Create(IXAudio2**, UINT32, XAUDIO2_PROCESSOR);

struct Completion final : IXAudio2VoiceCallback {
    HANDLE bufferEnd = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    HANDLE streamEnd = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    std::atomic<void*> returnedContext{nullptr};
    std::atomic<HRESULT> error{S_OK};

    ~Completion() {
        if (bufferEnd) CloseHandle(bufferEnd);
        if (streamEnd) CloseHandle(streamEnd);
    }

    void STDMETHODCALLTYPE OnBufferEnd(void* context) override {
        returnedContext.store(context);
        SetEvent(bufferEnd);
    }
    void STDMETHODCALLTYPE OnStreamEnd() override { SetEvent(streamEnd); }
    void STDMETHODCALLTYPE OnVoiceError(void*, HRESULT hr) override {
        error.store(hr);
        SetEvent(bufferEnd);
        SetEvent(streamEnd);
    }
    void STDMETHODCALLTYPE OnVoiceProcessingPassStart(UINT32) override {}
    void STDMETHODCALLTYPE OnVoiceProcessingPassEnd() override {}
    void STDMETHODCALLTYPE OnBufferStart(void*) override {}
    void STDMETHODCALLTYPE OnLoopEnd(void*) override {}
};

struct Audio {
    IXAudio2* engine = nullptr;
    IXAudio2MasteringVoice* master = nullptr;
    IXAudio2SourceVoice* source = nullptr;

    ~Audio() {
        if (source) source->DestroyVoice();
        if (master) master->DestroyVoice();
        if (engine) engine->Release();
    }
};

static bool failed(const char* operation, HRESULT hr) {
    if (SUCCEEDED(hr)) return false;
    char message[160];
    std::snprintf(message, sizeof(message), "PCM %s failed hr=0x%08lx", operation, static_cast<unsigned long>(hr));
    OutputDebugStringA(message);
    return true;
}

int main() {
    constexpr unsigned frames = 960;
    short samples[frames];
    for (unsigned i = 0; i < frames; ++i) {
        samples[i] = i % 48 < 24 ? 256 : -256;
    }
    XAUDIO2_BUFFER buffer{};
    buffer.Flags = XAUDIO2_END_OF_STREAM;
    buffer.AudioBytes = sizeof(samples);
    buffer.pAudioData = reinterpret_cast<const BYTE*>(samples);
    buffer.pContext = &buffer;

    Completion cb;
    Audio audio; // Destroy the voice before releasing its callback and PCM storage.
    if (!cb.bufferEnd || !cb.streamEnd) {
        OutputDebugStringA("PCM CreateEvent failed");
        return 1;
    }
    // Zero flags select the wrapped console engine, and processor zero selects its default.
    if (failed("XAudio2Create", GuestXAudio2Create(&audio.engine, 0, 0))) return 1;
    if (failed("CreateMasteringVoice", audio.engine->CreateMasteringVoice(&audio.master))) return 1;
    if (failed("SetVolume", audio.master->SetVolume(0.0f))) return 1;

    const WAVEFORMATEX format = {WAVE_FORMAT_PCM, 1, 48000, 96000, 2, 16, 0};
    if (failed("CreateSourceVoice", audio.engine->CreateSourceVoice(
            &audio.source, &format, 0, XAUDIO2_DEFAULT_FREQ_RATIO, &cb))) return 1;
    if (failed("SubmitSourceBuffer", audio.source->SubmitSourceBuffer(&buffer))) return 1;
    if (failed("Start", audio.source->Start())) return 1;

    HANDLE events[] = {cb.bufferEnd, cb.streamEnd};
    DWORD wait = WaitForMultipleObjects(2, events, TRUE, 5000);
    if (failed("OnVoiceError", cb.error.load())) return 1;

    void* returned = cb.returnedContext.load();
    bool streamEnded = WaitForSingleObject(cb.streamEnd, 0) == WAIT_OBJECT_0;
    char message[180];
    std::snprintf(message, sizeof(message), "PCM submitted_frames=%u context=%p OnBufferEnd=%p OnStreamEnd=%d wait=%lu",
                  frames, buffer.pContext, returned, streamEnded, wait);
    OutputDebugStringA(message);
    return wait == WAIT_OBJECT_0 && returned == buffer.pContext ? 0 : 1;
}
