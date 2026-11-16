#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <GL/gl.h>
#include "api/app/renderdoc_app.h"
#include <cstdint>
#include <iostream>
#include <string>
#include <vector>

static std::string utf8(const wchar_t *text)
{
  int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, -1, nullptr, 0, nullptr, nullptr);
  if(size <= 0) return {};
  std::vector<char> result(size_t(size), 0);
  if(WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, -1, result.data(), size, nullptr, nullptr) != size) return {};
  return result.data();
}

int wmain(int argc, wchar_t **argv)
{
  if(argc != 3) return 2;
  HMODULE rdoc = LoadLibraryW(argv[1]);
  auto get_api = rdoc ? reinterpret_cast<pRENDERDOC_GetAPI>(GetProcAddress(rdoc, "RENDERDOC_GetAPI")) : nullptr;
  RENDERDOC_API_1_7_0 *api = nullptr;
  if(!get_api || !get_api(eRENDERDOC_API_Version_1_7_0, reinterpret_cast<void **>(&api))) return 3;
  const std::string path = utf8(argv[2]);
  if(path.empty()) return 4;
  api->SetCaptureFilePathTemplate(path.c_str());
  WNDCLASSW wc = {}; wc.style = CS_OWNDC; wc.lpfnWndProc = DefWindowProcW; wc.hInstance = GetModuleHandleW(nullptr); wc.lpszClassName = L"RenderDocXOpenGLFixture";
  if(!RegisterClassW(&wc)) return 7;
  HWND window = CreateWindowW(wc.lpszClassName, L"fixture", WS_OVERLAPPEDWINDOW, 0, 0, 64, 64, nullptr, nullptr, wc.hInstance, nullptr);
  if(!window) return 8;
  HDC dc = GetDC(window);
  PIXELFORMATDESCRIPTOR pfd = {}; pfd.nSize = sizeof(pfd); pfd.nVersion = 1; pfd.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER; pfd.iPixelType = PFD_TYPE_RGBA; pfd.cColorBits = 32; pfd.cDepthBits = 24;
  int format = ChoosePixelFormat(dc, &pfd);
  if(!format || !SetPixelFormat(dc, format, &pfd)) return 9;
  HGLRC bootstrap = wglCreateContext(dc);
  if(!bootstrap || !wglMakeCurrent(dc, bootstrap)) return 10;
  auto create_context_attribs = reinterpret_cast<HGLRC (WINAPI *)(HDC, HGLRC, const int *)>(wglGetProcAddress("wglCreateContextAttribsARB"));
  if(!create_context_attribs) { std::cerr << "Modern OpenGL contexts unavailable"; return 10; }
  const int attributes[] = {0x2091, 3, 0x2092, 3, 0x9126, 1, 0};
  HGLRC context = create_context_attribs(dc, nullptr, attributes);
  wglMakeCurrent(nullptr, nullptr); wglDeleteContext(bootstrap);
  if(!context || !wglMakeCurrent(dc, context)) return 10;
  std::cerr << "OpenGL version: " << reinterpret_cast<const char *>(glGetString(GL_VERSION)) << '\n';
#define PROC(type, name) auto name = reinterpret_cast<type>(wglGetProcAddress(#name)); if(!name) { std::cerr << "Missing " #name; return 11; }
  PROC(GLuint (APIENTRY *)(GLenum), glCreateShader);
  PROC(void (APIENTRY *)(GLuint, GLsizei, const char *const *, const GLint *), glShaderSource);
  PROC(void (APIENTRY *)(GLuint), glCompileShader);
  PROC(void (APIENTRY *)(GLuint, GLenum, GLint *), glGetShaderiv);
  PROC(GLuint (APIENTRY *)(), glCreateProgram);
  PROC(void (APIENTRY *)(GLuint, GLuint), glAttachShader);
  PROC(void (APIENTRY *)(GLuint, GLuint, const char *), glBindAttribLocation);
  PROC(void (APIENTRY *)(GLuint), glLinkProgram);
  PROC(void (APIENTRY *)(GLuint, GLenum, GLint *), glGetProgramiv);
  PROC(void (APIENTRY *)(GLuint), glUseProgram);
  PROC(void (APIENTRY *)(GLsizei, GLuint *), glGenVertexArrays);
  PROC(void (APIENTRY *)(GLuint), glBindVertexArray);
  PROC(void (APIENTRY *)(GLsizei, GLuint *), glGenBuffers);
  PROC(void (APIENTRY *)(GLenum, GLuint), glBindBuffer);
  PROC(void (APIENTRY *)(GLenum, ptrdiff_t, const void *, GLenum), glBufferData);
  PROC(void (APIENTRY *)(GLuint), glEnableVertexAttribArray);
  PROC(void (APIENTRY *)(GLuint, GLint, GLenum, GLboolean, GLsizei, const void *), glVertexAttribPointer);
  PROC(void (APIENTRY *)(GLuint, GLfloat, GLfloat, GLfloat, GLfloat), glVertexAttrib4f);
  PROC(void (APIENTRY *)(GLuint, GLint, GLint, GLint, GLint), glVertexAttribI4i);
  PROC(void (APIENTRY *)(GLuint, GLuint, GLuint, GLuint, GLuint), glVertexAttribI4ui);
#undef PROC
  const char *vs = "#version 130\nin vec2 position; in vec4 tint; in ivec2 flags; in uvec2 counters; out vec4 color; void main(){ gl_Position=vec4(position,0,1); color=tint+vec4(float(flags.x+flags.y)*0.001,float(counters.x+counters.y)*0.001,0,0); }";
  const char *fs = "#version 130\nin vec4 color; out vec4 result; void main(){ result=color; }";
  GLuint vertex = glCreateShader(0x8B31), fragment = glCreateShader(0x8B30);
  glShaderSource(vertex, 1, &vs, nullptr); glCompileShader(vertex);
  glShaderSource(fragment, 1, &fs, nullptr); glCompileShader(fragment);
  GLint ok = 0; glGetShaderiv(vertex, 0x8B81, &ok); if(!ok) { std::cerr << "Vertex compilation failed"; return 12; }
  glGetShaderiv(fragment, 0x8B81, &ok); if(!ok) { std::cerr << "Fragment compilation failed"; return 13; }
  GLuint program = glCreateProgram(); glAttachShader(program, vertex); glAttachShader(program, fragment);
  glBindAttribLocation(program, 0, "position"); glBindAttribLocation(program, 1, "tint"); glBindAttribLocation(program, 2, "flags"); glBindAttribLocation(program, 3, "counters"); glLinkProgram(program);
  glGetProgramiv(program, 0x8B82, &ok); if(!ok) { std::cerr << "Program link failed"; return 14; }
  glUseProgram(program);
  GLuint vao = 0, buffer = 0; glGenVertexArrays(1, &vao); glBindVertexArray(vao); glGenBuffers(1, &buffer); glBindBuffer(0x8892, buffer);
  const int16_t vertices[] = {16384, -32768, 32767, 0, 0, 32767};
  glBufferData(0x8892, sizeof(vertices), vertices, 0x88E4);
  glEnableVertexAttribArray(0); glVertexAttribPointer(0, 2, GL_SHORT, GL_TRUE, 4, nullptr);
  glVertexAttrib4f(1, 0.25f, 0.5f, 0.75f, 1.0f);
  glVertexAttribI4i(2, -4, 42, 0, 1); glVertexAttribI4ui(3, 9, 123, 0, 1);
  api->StartFrameCapture(nullptr, nullptr);
  if(!api->IsFrameCapturing()) { std::cerr << "OpenGL capture did not start"; return 15; }
  glDrawArrays(GL_TRIANGLES, 0, 3); glFinish();
  if(!api->EndFrameCapture(nullptr, nullptr)) { std::cerr << "OpenGL capture did not complete"; return 16; }
  uint32_t length = 0;
  if(!api->GetCapture(0, nullptr, &length, nullptr) || !length) return 17;
  std::vector<char> output(length, 0);
  if(!api->GetCapture(0, output.data(), &length, nullptr)) return 18;
  std::cout << "RENDERDOCX_CAPTURE=" << output.data() << std::endl;
  wglMakeCurrent(nullptr, nullptr); wglDeleteContext(context); ReleaseDC(window, dc); DestroyWindow(window);
  return 0;
}
