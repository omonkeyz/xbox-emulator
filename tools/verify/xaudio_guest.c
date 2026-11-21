#include <windows.h>
#include <stdio.h>

__declspec(dllimport) DWORD WINAPI GetXAudioDefaultProcessorForCurrentPlatform(void);

int main(void)
{
    DWORD mask = GetXAudioDefaultProcessorForCurrentPlatform();
    char message[128];
    snprintf(message, sizeof(message),
             "guest pid=%lu GetXAudioDefaultProcessorForCurrentPlatform()=0x%08lx",
             GetCurrentProcessId(), mask);
    OutputDebugStringA(message);
    return mask == 0x20 ? 0 : 1;
}
