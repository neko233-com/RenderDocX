#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11.h>
#include <d3dcompiler.h>
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
  D3D11_TEXTURE2D_DESC layered_desc = desc;
  layered_desc.Width = 8; layered_desc.Height = 8; layered_desc.MipLevels = 2; layered_desc.ArraySize = 2;
  ID3D11Texture2D *layered = nullptr;
  ID3D11RenderTargetView *layered_views[4] = {};
  hr = device->CreateTexture2D(&layered_desc, nullptr, &layered);
  for(UINT slice = 0; SUCCEEDED(hr) && slice < 2; ++slice)
    for(UINT mip = 0; SUCCEEDED(hr) && mip < 2; ++mip)
    {
      D3D11_RENDER_TARGET_VIEW_DESC vd = {};
      vd.Format = layered_desc.Format; vd.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2DARRAY;
      vd.Texture2DArray.MipSlice = mip; vd.Texture2DArray.FirstArraySlice = slice; vd.Texture2DArray.ArraySize = 1;
      hr = device->CreateRenderTargetView(layered, &vd, &layered_views[slice * 2 + mip]);
    }
  if(FAILED(hr)) { std::cerr << "Could not create layered texture"; return 17; }
  D3D11_TEXTURE2D_DESC integer_desc = desc;
  integer_desc.Width = 4; integer_desc.Height = 4; integer_desc.Format = DXGI_FORMAT_R32G32B32A32_UINT;
  ID3D11Texture2D *integer_texture = nullptr;
  ID3D11RenderTargetView *integer_view = nullptr;
  hr = device->CreateTexture2D(&integer_desc, nullptr, &integer_texture);
  if(SUCCEEDED(hr)) hr = device->CreateRenderTargetView(integer_texture, nullptr, &integer_view);
  if(FAILED(hr)) { std::cerr << "Could not create integer texture"; return 18; }
  D3D11_TEXTURE3D_DESC volume_desc = {};
  volume_desc.Width = 4; volume_desc.Height = 4; volume_desc.Depth = 2; volume_desc.MipLevels = 1;
  volume_desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM; volume_desc.Usage = D3D11_USAGE_DEFAULT; volume_desc.BindFlags = D3D11_BIND_RENDER_TARGET;
  ID3D11Texture3D *volume = nullptr;
  ID3D11RenderTargetView *volume_views[2] = {};
  hr = device->CreateTexture3D(&volume_desc, nullptr, &volume);
  for(UINT slice = 0; SUCCEEDED(hr) && slice < 2; ++slice)
  {
    D3D11_RENDER_TARGET_VIEW_DESC vd = {};
    vd.Format = volume_desc.Format; vd.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE3D;
    vd.Texture3D.MipSlice = 0; vd.Texture3D.FirstWSlice = slice; vd.Texture3D.WSize = 1;
    hr = device->CreateRenderTargetView(volume, &vd, &volume_views[slice]);
  }
  if(FAILED(hr)) { std::cerr << "Could not create 3D texture"; return 19; }

  HMODULE compiler = LoadLibraryW(L"d3dcompiler_47.dll");
  auto compile = compiler ? reinterpret_cast<decltype(&D3DCompile)>(GetProcAddress(compiler, "D3DCompile")) : nullptr;
  if(!compile) { std::cerr << "Could not load D3D compiler"; return 14; }
  const char *vertex_source = "float4 main(float2 position : POSITION) : SV_Position { return float4(position,0,1); }";
  const char *pixel_source = "float4 main() : SV_Target { return float4(0.25,0.5,0.75,1); }";
  ID3DBlob *vertex_code = nullptr, *pixel_code = nullptr;
  hr = compile(vertex_source, strlen(vertex_source), nullptr, nullptr, nullptr, "main", "vs_4_0", 0, 0, &vertex_code, nullptr);
  if(SUCCEEDED(hr)) hr = compile(pixel_source, strlen(pixel_source), nullptr, nullptr, nullptr, "main", "ps_4_0", 0, 0, &pixel_code, nullptr);
  ID3D11VertexShader *vertex = nullptr;
  ID3D11PixelShader *pixel = nullptr;
  ID3D11InputLayout *layout = nullptr;
  D3D11_INPUT_ELEMENT_DESC element = {"POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0};
  if(SUCCEEDED(hr)) hr = device->CreateVertexShader(vertex_code->GetBufferPointer(), vertex_code->GetBufferSize(), nullptr, &vertex);
  if(SUCCEEDED(hr)) hr = device->CreateInputLayout(&element, 1, vertex_code->GetBufferPointer(), vertex_code->GetBufferSize(), &layout);
  if(SUCCEEDED(hr)) hr = device->CreatePixelShader(pixel_code->GetBufferPointer(), pixel_code->GetBufferSize(), nullptr, &pixel);
  if(FAILED(hr)) { std::cerr << "Could not create fixture shaders"; return 15; }
  vertex_code->Release(); pixel_code->Release();
  const float vertices[] = {0, 2, 2, 2, 2, 2, 2};
  const uint16_t indices[] = {0, 0, 1, 2};
  D3D11_BUFFER_DESC buffer_desc = {};
  buffer_desc.ByteWidth = sizeof(vertices); buffer_desc.Usage = D3D11_USAGE_DEFAULT; buffer_desc.BindFlags = D3D11_BIND_VERTEX_BUFFER;
  D3D11_SUBRESOURCE_DATA initial = {}; initial.pSysMem = vertices;
  ID3D11Buffer *vertex_buffer = nullptr, *index_buffer = nullptr;
  hr = device->CreateBuffer(&buffer_desc, &initial, &vertex_buffer);
  buffer_desc.ByteWidth = sizeof(indices); buffer_desc.BindFlags = D3D11_BIND_INDEX_BUFFER; initial.pSysMem = indices;
  if(SUCCEEDED(hr)) hr = device->CreateBuffer(&buffer_desc, &initial, &index_buffer);
  if(FAILED(hr)) { std::cerr << "Could not create fixture buffers"; return 16; }

  api->StartFrameCapture(device, nullptr);
  if(!api->IsFrameCapturing()) { std::cerr << "Capture did not start"; return 9; }
  const float color[4] = {0.25f, 0.5f, 0.75f, 1.0f};
  context->ClearRenderTargetView(view, color);
  const float integer_color[4] = {1, 2, 3, 4};
  context->ClearRenderTargetView(integer_view, integer_color);
  const float volume_colors[2][4] = {{1,0,0,1}, {0,0,1,1}};
  for(unsigned i = 0; i < 2; ++i) context->ClearRenderTargetView(volume_views[i], volume_colors[i]);
  const float layer_colors[4][4] = {{1,0,0,1}, {0,1,0,1}, {0,0,1,1}, {1,1,0,1}};
  for(unsigned i = 0; i < 4; ++i) context->ClearRenderTargetView(layered_views[i], layer_colors[i]);
  const float updated_vertices[] = {0, 3, 3, 3, 3, 3, 3};
  context->UpdateSubresource(vertex_buffer, 0, nullptr, updated_vertices, 0, 0);
  context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
  const UINT stride = 8, offset = 4;
  context->IASetInputLayout(layout);
  context->IASetVertexBuffers(0, 1, &vertex_buffer, &stride, &offset);
  context->IASetIndexBuffer(index_buffer, DXGI_FORMAT_R16_UINT, 2);
  context->VSSetShader(vertex, nullptr, 0);
  context->PSSetShader(pixel, nullptr, 0);
  context->OMSetRenderTargets(1, &view, nullptr);
  D3D11_VIEWPORT viewport = {0, 0, 16, 16, 0, 1};
  context->RSSetViewports(1, &viewport);
  context->DrawIndexed(3, 0, 0);
  context->Flush();
  if(!api->EndFrameCapture(device, nullptr)) { std::cerr << "Capture did not complete"; return 10; }
  if(api->GetNumCaptures() == 0) { std::cerr << "No capture was recorded"; return 11; }
  uint32_t length = 0;
  if(!api->GetCapture(0, nullptr, &length, nullptr) || length == 0) { std::cerr << "Could not read capture path"; return 12; }
  std::vector<char> output(length, 0);
  if(!api->GetCapture(0, output.data(), &length, nullptr)) { std::cerr << "Could not read capture path"; return 13; }
  std::cout << "RENDERDOCX_CAPTURE=" << output.data() << std::endl;
  for(auto *layer_view : layered_views) layer_view->Release();
  for(auto *volume_view : volume_views) volume_view->Release();
  volume->Release(); integer_view->Release(); integer_texture->Release(); layered->Release(); index_buffer->Release(); vertex_buffer->Release(); layout->Release(); vertex->Release(); pixel->Release(); view->Release(); texture->Release(); context->Release(); device->Release();
  return 0;
}
