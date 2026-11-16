#include "api/replay/renderdoc_replay.h"
#include "api/replay/version.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <set>
#include <string>
#include <vector>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif

// Prevent RenderDoc from attempting to capture the replay helper itself.
REPLAY_PROGRAM_MARKER();

namespace
{
void json_string(const char *value, size_t length)
{
  static const char hex[] = "0123456789abcdef";
  std::cout << '"';
  for(size_t i = 0; i < length; ++i)
  {
    const unsigned char c = static_cast<unsigned char>(value[i]);
    switch(c)
    {
      case '"': std::cout << "\\\""; break;
      case '\\': std::cout << "\\\\"; break;
      case '\b': std::cout << "\\b"; break;
      case '\f': std::cout << "\\f"; break;
      case '\n': std::cout << "\\n"; break;
      case '\r': std::cout << "\\r"; break;
      case '\t': std::cout << "\\t"; break;
      default:
        if(c < 0x20)
          std::cout << "\\u00" << hex[c >> 4] << hex[c & 15];
        else
          std::cout << static_cast<char>(c);
    }
  }
  std::cout << '"';
}

void json_string(const rdcstr &value) { json_string(value.c_str(), value.size()); }
void error(const std::string &message)
{
  std::cout << "\nRENDERDOCX/1 {\"type\":\"error\",\"message\":";
  json_string(message.data(), message.size());
  std::cout << "}" << std::endl;
}

std::string result_message(const ResultDetails &result)
{
  if(result.internal_msg) return std::string(result.internal_msg->c_str(), result.internal_msg->size());
  return "RenderDoc error code " + std::to_string(uint32_t(result.code));
}

struct ReplayScope
{
  ICaptureFile *file = nullptr;
  IReplayController *controller = nullptr;
  bool initialised = false;
  ~ReplayScope()
  {
    if(controller) controller->Shutdown();
    if(file) file->Shutdown();
    if(initialised) RENDERDOC_ShutdownReplay();
  }
};

struct PendingAction
{
  const ActionDescription *action;
  size_t depth;
  int64_t parent;
};

void write_snapshot(IReplayController *controller, std::set<uint32_t> &event_ids)
{
  const APIProperties api = controller->GetAPIProperties();
  const FrameDescription frame = controller->GetFrameInfo();
  const SDFile &structured = controller->GetStructuredFile();
  const auto &roots = controller->GetRootActions();
  std::vector<PendingAction> pending;
  for(size_t i = roots.size(); i > 0; --i) pending.push_back({&roots[i - 1], 0, -1});

  std::cout << "\nRENDERDOCX/1 {\"type\":\"snapshot\",\"protocol\":1,\"api\":" << uint32_t(api.pipelineType)
            << ",\"frame\":" << frame.frameNumber << ",\"degraded\":"
            << (api.degraded ? "true" : "false") << ",\"actions\":[";
  size_t index = 0;
  while(!pending.empty())
  {
    const PendingAction node = pending.back();
    pending.pop_back();
    const ActionDescription &action = *node.action;
    if(index) std::cout << ',';
    std::cout << "{\"event_id\":" << action.eventId << ",\"action_id\":" << action.actionId
              << ",\"depth\":" << node.depth << ",\"parent\":" << node.parent
              << ",\"flags\":" << uint32_t(action.flags) << ",\"indices\":" << action.numIndices
              << ",\"instances\":" << action.numInstances << ",\"name\":";
    json_string(action.GetName(structured));
    std::cout << '}';
    event_ids.insert(action.eventId);
    for(size_t i = action.children.size(); i > 0; --i)
      pending.push_back({&action.children[i - 1], node.depth + 1, static_cast<int64_t>(index)});
    ++index;
  }
  const auto &resources = controller->GetResources();
  const auto &textures = controller->GetTextures();
  const auto &buffers = controller->GetBuffers();
  std::cout << "],\"resource_count\":" << resources.size() << ",\"texture_count\":"
            << textures.size() << ",\"buffer_count\":" << buffers.size() << "}" << std::endl;
}

int run(const std::string &capture)
{
  const char *version = RENDERDOC_GetVersionString();
  const char *commit = RENDERDOC_GetCommitHash();
  if(!version || std::strcmp(version, MAJOR_MINOR_VERSION_STRING) != 0 || !commit ||
     std::strcmp(commit, RENDERDOCX_CORE_COMMIT) != 0)
  {
    error("RenderDoc core does not match the version and commit used to build this bridge");
    return 1;
  }

  ReplayScope replay;
  GlobalEnvironment environment;
  RENDERDOC_InitialiseReplay(environment, {});
  replay.initialised = true;
  replay.file = RENDERDOC_OpenCaptureFile();
  if(!replay.file) { error("RenderDoc could not create a capture file handle"); return 1; }
  ResultDetails result = replay.file->OpenFile(capture.c_str(), "rdc", nullptr);
  if(!result.OK()) { error(result_message(result)); return 1; }
  auto opened = replay.file->OpenCapture(ReplayOptions(), nullptr);
  if(!opened.first.OK()) { error(result_message(opened.first)); return 1; }
  replay.controller = opened.second;
  if(!replay.controller) { error("RenderDoc returned no replay controller"); return 1; }
  replay.file->Shutdown();
  replay.file = nullptr;
  std::set<uint32_t> event_ids;
  write_snapshot(replay.controller, event_ids);

  std::string command;
  while(std::getline(std::cin, command))
  {
    if(command == "quit") break;
    if(command.compare(0, 7, "select ") != 0) { error("Unknown command"); continue; }
    const std::string number = command.substr(7);
    uint64_t event = 0;
    bool valid = !number.empty();
    for(char c : number)
    {
      if(c < '0' || c > '9') { valid = false; break; }
      event = event * 10 + uint64_t(c - '0');
      if(event > (std::numeric_limits<uint32_t>::max)()) { valid = false; break; }
    }
    if(!valid || event_ids.count(uint32_t(event)) == 0) { error("Unknown action event"); continue; }
    replay.controller->SetFrameEvent(uint32_t(event), false);
    result = replay.controller->GetFatalErrorStatus();
    if(!result.OK()) { error(result_message(result)); break; }
    std::cout << "\nRENDERDOCX/1 {\"type\":\"selected\",\"event_id\":" << event << "}" << std::endl;
  }
  return 0;
}
} // namespace

#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
{
  if(argc != 2) { error("Expected one capture file path"); return 2; }
  const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[1], -1, nullptr, 0, nullptr, nullptr);
  if(size <= 0) { error("Capture path is not valid Unicode"); return 2; }
  std::vector<char> path(size_t(size), 0);
  if(WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[1], -1, path.data(), size, nullptr, nullptr) != size)
  { error("Could not convert capture path"); return 2; }
  return run(path.data());
}
#else
int main(int argc, char **argv)
{
  if(argc != 2) { error("Expected one capture file path"); return 2; }
  return run(argv[1]);
}
#endif
