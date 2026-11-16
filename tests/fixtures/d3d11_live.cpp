#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11.h>
#include "api/app/renderdoc_app.h"
#include <filesystem>
#include <fstream>

int wmain(int argc, wchar_t **argv)
{
  if(argc != 2) return 2;
  const std::filesystem::path stop(argv[1]);
  const std::filesystem::path done(stop.wstring() + L".done");
  HMODULE renderdoc = GetModuleHandleW(L"renderdoc.dll");
  auto get_api = renderdoc ? reinterpret_cast<pRENDERDOC_GetAPI>(GetProcAddress(renderdoc, "RENDERDOC_GetAPI")) : nullptr;
  RENDERDOC_API_1_7_0 *api = nullptr;
  if(!get_api || !get_api(eRENDERDOC_API_Version_1_7_0, reinterpret_cast<void **>(&api))) return 8;
  wchar_t marker[128] = {};
  GetEnvironmentVariableW(L"RENDERDOCX_FIXTURE_MARKER", marker, 128);
  std::ofstream info(std::filesystem::path(stop.wstring() + L".info"));
  info << (std::filesystem::equivalent(std::filesystem::current_path(), stop.parent_path()) ? 1 : 0) << ' '
       << (std::wstring(marker) == L"开始:中间:结束" ? 1 : 0) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_AllowVSync) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_AllowFullscreen) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_CaptureCallstacks) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_CaptureCallstacksOnlyActions) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_RefAllResources) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_DebugOutputMute) << ' '
       << api->GetCaptureOptionU32(eRENDERDOC_Option_SoftMemoryLimit);
  info.close();
  WNDCLASSW wc = {};
  wc.lpfnWndProc = DefWindowProcW; wc.hInstance = GetModuleHandleW(nullptr); wc.lpszClassName = L"RenderDocXLiveFixture";
  if(!RegisterClassW(&wc)) return 3;
  HWND window = CreateWindowW(wc.lpszClassName, L"RenderDocX fixture", WS_OVERLAPPEDWINDOW,
                              0, 0, 32, 32, nullptr, nullptr, wc.hInstance, nullptr);
  if(!window) return 4;
  HMODULE d3d = LoadLibraryW(L"d3d11.dll");
  auto create = d3d ? reinterpret_cast<decltype(&D3D11CreateDeviceAndSwapChain)>(GetProcAddress(d3d, "D3D11CreateDeviceAndSwapChain")) : nullptr;
  if(!create) return 5;
  DXGI_SWAP_CHAIN_DESC desc = {};
  desc.BufferDesc.Width = 16; desc.BufferDesc.Height = 16; desc.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
  desc.SampleDesc.Count = 1; desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT; desc.BufferCount = 1;
  desc.OutputWindow = window; desc.Windowed = TRUE; desc.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;
  ID3D11Device *device = nullptr; ID3D11DeviceContext *context = nullptr; IDXGISwapChain *swap = nullptr;
  D3D_FEATURE_LEVEL feature;
  HRESULT hr = create(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, nullptr, 0, D3D11_SDK_VERSION, &desc, &swap, &device, &feature, &context);
  if(FAILED(hr)) hr = create(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, 0, nullptr, 0, D3D11_SDK_VERSION, &desc, &swap, &device, &feature, &context);
  if(FAILED(hr)) return 6;
  ID3D11Texture2D *texture = nullptr; ID3D11RenderTargetView *view = nullptr;
  hr = swap->GetBuffer(0, __uuidof(ID3D11Texture2D), reinterpret_cast<void **>(&texture));
  if(SUCCEEDED(hr)) hr = device->CreateRenderTargetView(texture, nullptr, &view);
  if(FAILED(hr)) return 7;
  const float color[4] = {0.2f, 0.4f, 0.6f, 1.0f};
  for(unsigned i = 0; i < 4000 && !std::filesystem::exists(stop); ++i)
  {
    context->ClearRenderTargetView(view, color);
    if(FAILED(swap->Present(0, 0))) break;
    MSG msg;
    while(PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
    Sleep(15);
  }
  view->Release(); texture->Release(); swap->Release(); context->Release(); device->Release();
  DestroyWindow(window);
  std::ofstream(done).put('1');
  return 0;
}
