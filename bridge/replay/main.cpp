#include "api/replay/renderdoc_replay.h"
#include "api/replay/version.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <map>
#include <set>
#include <string>
#include <vector>

// pipestate.inl uses this non-exported stringiser for vertex attribute names.
template <> rdcstr DoStringise<uint32_t>(const uint32_t &value)
{
  return std::to_string(value).c_str();
}
#include "api/replay/pipestate.inl"

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

void write_snapshot(IReplayController *controller, std::set<uint32_t> &event_ids,
                    std::vector<ResourceId> &texture_ids)
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
  std::map<ResourceId, const rdcstr *> resource_names;
  for(const ResourceDescription &resource : resources) resource_names.emplace(resource.resourceId, &resource.name);
  std::cout << "],\"resource_count\":" << resources.size() << ",\"texture_count\":"
            << textures.size() << ",\"buffer_count\":" << buffers.size() << ",\"textures\":[";
  for(size_t i = 0; i < textures.size(); ++i)
  {
    const TextureDescription &texture = textures[i];
    texture_ids.push_back(texture.resourceId);
    const auto found_name = resource_names.find(texture.resourceId);
    const rdcstr *name = found_name == resource_names.end() ? nullptr : found_name->second;
    if(i) std::cout << ',';
    std::cout << "{\"name\":";
    if(name) json_string(*name); else json_string("", 0);
    std::cout << ",\"width\":" << texture.width << ",\"height\":" << texture.height
              << ",\"depth\":" << texture.depth << ",\"dimension\":" << texture.dimension
              << ",\"mips\":" << texture.mips << ",\"array_size\":" << texture.arraysize
              << ",\"samples\":" << texture.msSamp << ",\"bytes\":\"" << texture.byteSize
              << "\",\"format_name\":";
    json_string(texture.format.Name());
    std::cout << ",\"format_type\":" << uint32_t(texture.format.type)
              << ",\"component_type\":" << uint32_t(texture.format.compType)
              << ",\"component_count\":" << uint32_t(texture.format.compCount)
              << ",\"component_byte_width\":" << uint32_t(texture.format.compByteWidth)
              << '}';
  }
  std::cout << "]}" << std::endl;
}

// ResourceId is opaque. Indices refer to GetResources() in this replay session only.
void write_pipeline(IReplayController *controller, uint32_t event)
{
  const PipeState &state = controller->GetPipelineState();
  const auto &resources = controller->GetResources();
  std::map<ResourceId, size_t> indices;
  for(size_t i = 0; i < resources.size(); ++i) indices.emplace(resources[i].resourceId, i);
  auto resource = [&](ResourceId id) {
    const auto found = indices.find(id);
    std::cout << "\"bound\":" << (id != ResourceId() ? "true" : "false") << ",\"resource_index\":";
    if(found == indices.end() || id == ResourceId()) std::cout << "null";
    else std::cout << found->second;
    std::cout << ",\"name\":";
    if(found == indices.end() || id == ResourceId()) json_string("", 0);
    else json_string(resources[found->second].name);
  };
  static const char *names[] = {"vertex", "tess_control", "tess_eval", "geometry", "fragment", "compute", "task", "mesh"};
  std::cout << "{\"event_id\":" << event << ",\"available\":" << (state.IsCaptureLoaded() ? "true" : "false")
            << ",\"topology\":" << uint32_t(state.GetPrimitiveTopology()) << ",\"shaders\":[";
  for(unsigned i = 0; i < 8; ++i)
  {
    if(i) std::cout << ',';
    std::cout << "{\"stage\":";
    json_string(names[i], std::strlen(names[i]));
    std::cout << ',';
    resource(state.GetShader(ShaderStage(i)));
    std::cout << '}';
  }
  std::cout << "],\"outputs\":[";
  const auto outputs = state.GetOutputTargets();
  for(size_t i = 0; i < outputs.size(); ++i)
  {
    if(i) std::cout << ',';
    std::cout << '{';
    resource(outputs[i].resource);
    std::cout << '}';
  }
  std::cout << "]}";
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
  std::vector<ResourceId> texture_ids;
  write_snapshot(replay.controller, event_ids, texture_ids);

  std::string command;
  while(std::getline(std::cin, command))
  {
    if(command == "quit") break;
    if(command.compare(0, 5, "save ") == 0)
    {
      const size_t separator = command.find(' ', 5);
      if(separator == std::string::npos) { error("Invalid texture save command"); continue; }
      const std::string number = command.substr(5, separator - 5);
      const std::string encoded = command.substr(separator + 1);
      uint64_t index = 0;
      bool valid = !number.empty() && !encoded.empty() && encoded.size() % 2 == 0;
      for(char c : number)
      {
        if(c < '0' || c > '9' || index > (std::numeric_limits<uint32_t>::max)() / 10)
        { valid = false; break; }
        index = index * 10 + uint64_t(c - '0');
        if(index > (std::numeric_limits<uint32_t>::max)()) { valid = false; break; }
      }
      std::string path;
      for(size_t i = 0; valid && i < encoded.size(); i += 2)
      {
        auto digit = [](char c) -> int { return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1; };
        int high = digit(encoded[i]), low = digit(encoded[i + 1]);
        if(high < 0 || low < 0 || (high == 0 && low == 0)) { valid = false; break; }
        path.push_back(char(high * 16 + low));
      }
      if(!valid || index >= texture_ids.size()) { error("Invalid texture or output path"); continue; }
      TextureSave save;
      save.resourceId = texture_ids[size_t(index)];
      save.destType = FileType::PNG;
      ResultDetails saved = replay.controller->SaveTexture(save, path.c_str());
      if(!saved.OK()) { error(result_message(saved)); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"texture_saved\",\"index\":" << index << "}" << std::endl;
      continue;
    }
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
    std::cout << "\nRENDERDOCX/1 {\"type\":\"selected\",\"event_id\":" << event << ",\"pipeline\":";
    write_pipeline(replay.controller, uint32_t(event));
    std::cout << "}" << std::endl;
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
