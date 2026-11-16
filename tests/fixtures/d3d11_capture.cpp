#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11.h>
#include "api/app/renderdoc_app.h"
#include <iostream>
#include <string>
#include <vector>

static std::string utf8(const wchar_t *text)
{
  int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, -1, nullptr, 0, nullptr, nullptr);
  if(size <= 0) return {};
  std::vector<char> result(size_t(size), 0);
  if(WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, -1, result.data(), size, nullptr, nullptr) != size)
    return {};
  return result.data();
}

int wmain(int argc, wchar_t **argv)
{
  if(argc != 3) { std::cerr << "Expected renderdoc.dll and capture path template"; return 2; }
  HMODULE renderdoc = LoadLibraryW(argv[1]);
  if(!renderdoc) { std::cerr << "Could not load RenderDoc"; return 3; }
  auto get_api = reinterpret_cast<pRENDERDOC_GetAPI>(GetProcAddress(renderdoc, "RENDERDOC_GetAPI"));
  RENDERDOC_API_1_7_0 *api = nullptr;
  if(!get_api || !get_api(eRENDERDOC_API_Version_1_7_0, reinterpret_cast<void **>(&api)))
  { std::cerr << "Could not obtain RenderDoc application API"; return 4; }
  const std::string path = utf8(argv[2]);
  if(path.empty()) { std::cerr << "Invalid capture path"; return 5; }
  api->SetCaptureFilePathTemplate(path.c_str());

  HMODULE d3d = LoadLibraryW(L"d3d11.dll");
  auto create = d3d ? reinterpret_cast<decltype(&D3D11CreateDevice)>(GetProcAddress(d3d, "D3D11CreateDevice")) : nullptr;
  if(!create) { std::cerr << "Could not load D3D11"; return 6; }
  ID3D11Device *device = nullptr;
  ID3D11DeviceContext *context = nullptr;
  D3D_FEATURE_LEVEL feature;
  HRESULT hr = create(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, nullptr, 0, D3D11_SDK_VERSION, &device, &feature, &context);
  if(FAILED(hr)) hr = create(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, 0, nullptr, 0, D3D11_SDK_VERSION, &device, &feature, &context);
  if(FAILED(hr)) { std::cerr << "Could not create a D3D11 device"; return 7; }
  D3D11_TEXTURE2D_DESC desc = {};
  desc.Width = 16; desc.Height = 16; desc.MipLevels = 1; desc.ArraySize = 1;
  desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM; desc.SampleDesc.Count = 1;
  desc.Usage = D3D11_USAGE_DEFAULT; desc.BindFlags = D3D11_BIND_RENDER_TARGET;
  ID3D11Texture2D *texture = nullptr;
  ID3D11RenderTargetView *view = nullptr;
  hr = device->CreateTexture2D(&desc, nullptr, &texture);
  if(SUCCEEDED(hr)) hr = device->CreateRenderTargetView(texture, nullptr, &view);
  if(FAILED(hr)) { std::cerr << "Could not create render target"; return 8; }

  api->StartFrameCapture(device, nullptr);
  if(!api->IsFrameCapturing()) { std::cerr << "Capture did not start"; return 9; }
  const float color[4] = {0.25f, 0.5f, 0.75f, 1.0f};
  context->ClearRenderTargetView(view, color);
  context->Flush();
  if(!api->EndFrameCapture(device, nullptr)) { std::cerr << "Capture did not complete"; return 10; }
  if(api->GetNumCaptures() == 0) { std::cerr << "No capture was recorded"; return 11; }
  uint32_t length = 0;
  if(!api->GetCapture(0, nullptr, &length, nullptr) || length == 0) { std::cerr << "Could not read capture path"; return 12; }
  std::vector<char> output(length, 0);
  if(!api->GetCapture(0, output.data(), &length, nullptr)) { std::cerr << "Could not read capture path"; return 13; }
  std::cout << "RENDERDOCX_CAPTURE=" << output.data() << std::endl;
  view->Release(); texture->Release(); context->Release(); device->Release();
  return 0;
}
