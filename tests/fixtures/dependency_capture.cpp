#include "api/replay/renderdoc_replay.h"
#include <cstdint>
#include <string>
#include <vector>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif
REPLAY_PROGRAM_MARKER();

int run(const char *path)
{
  GlobalEnvironment env;
  RENDERDOC_InitialiseReplay(env, {});
  ICaptureFile *file = RENDERDOC_OpenCaptureFile();
  if(!file) return 1;
  ResultDetails opened = file->OpenFile(path, "rdc", nullptr);
  if(!opened.OK()) { file->Shutdown(); RENDERDOC_ShutdownReplay(); return 2; }
  bytebuf bytes;
  const uint32_t count = 1, name_size = 18, data_size = 12;
  const char name[] = "renderdocx-fixture";
  const char data[] = "fixture-data";
  bytes.append((const byte *)&count, sizeof(count));
  bytes.append((const byte *)&name_size, sizeof(name_size));
  bytes.append((const byte *)name, name_size);
  bytes.append((const byte *)&data_size, sizeof(data_size));
  bytes.append((const byte *)data, data_size);
  SectionProperties properties; properties.type = SectionType::EmbeddedExternalFiles; properties.version = 1;
  ResultDetails saved = file->WriteSection(properties, bytes);
  const bool ok = saved.OK();
  file->Shutdown(); RENDERDOC_ShutdownReplay(); return ok ? 0 : 3;
}
#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
{
  if(argc != 2) return 4;
  const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[1], -1, nullptr, 0, nullptr, nullptr);
  if(size <= 0) return 4;
  std::vector<char> path(size_t(size), 0);
  if(!WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[1], -1, path.data(), size, nullptr, nullptr)) return 4;
  return run(path.data());
}
#else
int main(int argc, char **argv) { return argc == 2 ? run(argv[1]) : 4; }
#endif
