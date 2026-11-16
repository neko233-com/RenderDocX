#include "api/replay/renderdoc_replay.h"
#include "api/replay/version.h"

#include <cstdint>
#include <algorithm>
#include <cstring>
#include <cmath>
#include <iomanip>
#include <iostream>
#include <limits>
#include <map>
#include <set>
#include <sstream>
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

struct BufferRef { ResourceId id; uint64_t length; };
struct TextureRef { ResourceId id; uint32_t dimension, depth, mips, arraysize, samples, components; };

bool decimal(const std::string &text, uint64_t &value)
{
  value = 0;
  if(text.empty()) return false;
  for(char c : text)
  {
    if(c < '0' || c > '9') return false;
    const uint64_t digit = uint64_t(c - '0');
    if(value > ((std::numeric_limits<uint64_t>::max)() - digit) / 10) return false;
    value = value * 10 + digit;
  }
  return true;
}

void write_snapshot(IReplayController *controller, std::set<uint32_t> &event_ids,
                    std::vector<TextureRef> &texture_ids, std::vector<BufferRef> &buffer_ids,
                    std::map<uint32_t, uint32_t> &event_chunks)
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
    std::cout << ",\"api_events\":[";
    for(size_t i = 0; i < action.events.size(); ++i)
    {
      const APIEvent &event = action.events[i];
      if(i) std::cout << ',';
      std::cout << "{\"event_id\":" << event.eventId << ",\"chunk_index\":";
      if(event.chunkIndex == APIEvent::NoChunk) std::cout << "null";
      else std::cout << event.chunkIndex;
      std::cout << ",\"file_offset\":\"" << event.fileOffset << "\",\"name\":";
      if(event.chunkIndex < structured.chunks.size() && structured.chunks[event.chunkIndex])
      {
        rdcstr name = structured.chunks[event.chunkIndex]->name;
        name += "()";
        json_string(name);
      }
      else json_string("", 0);
      std::cout << '}';
      event_ids.insert(event.eventId);
      if(event.chunkIndex < structured.chunks.size() && structured.chunks[event.chunkIndex])
        event_chunks.emplace(event.eventId, event.chunkIndex);
    }
    std::cout << "]}";
    event_ids.insert(action.eventId);
    for(size_t i = action.children.size(); i > 0; --i)
      pending.push_back({&action.children[i - 1], node.depth + 1, static_cast<int64_t>(index)});
    ++index;
  }
  const auto &resources = controller->GetResources();
  const auto &textures = controller->GetTextures();
  const auto &buffers = controller->GetBuffers();
  std::map<ResourceId, const rdcstr *> resource_names;
  std::map<ResourceId, size_t> resource_indices;
  for(size_t i = 0; i < resources.size(); ++i)
  {
    resource_names.emplace(resources[i].resourceId, &resources[i].name);
    resource_indices.emplace(resources[i].resourceId, i);
  }
  std::cout << "],\"resource_count\":" << resources.size() << ",\"texture_count\":"
            << textures.size() << ",\"buffer_count\":" << buffers.size() << ",\"textures\":[";
  for(size_t i = 0; i < textures.size(); ++i)
  {
    const TextureDescription &texture = textures[i];
    texture_ids.push_back({texture.resourceId, texture.dimension, texture.depth, texture.mips, texture.arraysize, texture.msSamp, texture.format.compCount});
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
  std::cout << "],\"buffers\":[";
  for(size_t i = 0; i < buffers.size(); ++i)
  {
    const BufferDescription &buffer = buffers[i];
    buffer_ids.push_back({buffer.resourceId, buffer.length});
    const auto name = resource_names.find(buffer.resourceId);
    const auto resource_index = resource_indices.find(buffer.resourceId);
    if(i) std::cout << ',';
    std::cout << "{\"name\":";
    if(name == resource_names.end()) json_string("", 0); else json_string(*name->second);
    std::cout << ",\"resource_index\":";
    if(resource_index == resource_indices.end()) std::cout << "null"; else std::cout << resource_index->second;
    std::cout << ",\"length\":\"" << buffer.length << "\",\"creation_flags\":" << uint32_t(buffer.creationFlags) << '}';
  }
  std::cout << "]}" << std::endl;
}

// Preserve upstream type metadata and scalar values; child arrays are paged separately.
void write_structured_node(const SDObject *object, const std::map<ResourceId, size_t> &resources)
{
  std::cout << "{\"name\":"; json_string(object->name.c_str(), object->name.size());
  std::cout << ",\"type_name\":"; json_string(object->type.name.c_str(), object->type.name.size());
  std::cout << ",\"basic_type\":" << uint32_t(object->type.basetype)
            << ",\"flags\":" << uint32_t(object->type.flags)
            << ",\"byte_size\":\"" << object->type.byteSize << "\",\"child_count\":\""
            << object->NumChildren() << "\",\"value\":";
  std::string scalar;
  bool has_scalar = true;
  switch(object->type.basetype)
  {
    case SDBasic::String:
      if(object->type.flags & SDTypeFlags::NullString) has_scalar = false;
      else scalar.assign(object->data.str.c_str(), object->data.str.size());
      break;
    case SDBasic::Enum:
    case SDBasic::UnsignedInteger:
    case SDBasic::GPUAddress: scalar = std::to_string(object->data.basic.u); break;
    case SDBasic::SignedInteger: scalar = std::to_string(object->data.basic.i); break;
    case SDBasic::Float:
    {
      std::ostringstream stream;
      stream << std::setprecision(std::numeric_limits<double>::max_digits10) << object->data.basic.d;
      scalar = stream.str();
      break;
    }
    case SDBasic::Boolean: scalar = object->data.basic.b ? "true" : "false"; break;
    case SDBasic::Character: scalar = std::to_string(static_cast<unsigned char>(object->data.basic.c)); break;
    default: has_scalar = false; break;
  }
  if(has_scalar) json_string(scalar.data(), scalar.size()); else std::cout << "null";
  std::cout << ",\"custom_string\":";
  if(object->type.flags & SDTypeFlags::HasCustomString) json_string(object->data.str.c_str(), object->data.str.size());
  else std::cout << "null";
  std::cout << ",\"resource_non_null\":";
  if(object->type.basetype == SDBasic::Resource) std::cout << (object->AsResourceId() == ResourceId() ? "false" : "true");
  else std::cout << "null";
  std::cout << ",\"resource_index\":";
  if(object->type.basetype == SDBasic::Resource)
  {
    auto found = resources.find(object->AsResourceId());
    if(found != resources.end() && object->AsResourceId() != ResourceId()) std::cout << found->second;
    else std::cout << "null";
  }
  else std::cout << "null";
  std::cout << '}';
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
  auto buffer = [&](const BoundVBuffer &binding) {
    std::cout << '{';
    resource(binding.resourceId);
    // Preserve 64-bit values for JavaScript consumers, including upstream sentinels.
    std::cout << ",\"byte_offset\":\"" << binding.byteOffset << "\",\"byte_size\":\""
              << binding.byteSize << "\",\"byte_stride\":" << binding.byteStride << '}';
  };
  auto number = [](float value) {
    if(std::isfinite(value)) std::cout << std::setprecision(std::numeric_limits<float>::max_digits10) << value;
    else std::cout << "null";
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
  std::cout << "],\"index_buffer\":";
  buffer(state.GetIBuffer());
  std::cout << ",\"vertex_buffers\":[";
  const auto vertex_buffers = state.GetVBuffers();
  for(size_t i = 0; i < vertex_buffers.size(); ++i)
  {
    if(i) std::cout << ',';
    buffer(vertex_buffers[i]);
  }
  std::cout << "],\"vertex_inputs\":";
  // GetVertexInputs() can return uninitialised trailing attributes for OpenGL.
  if(!state.IsCaptureLoaded() || state.IsCaptureGL()) std::cout << "null";
  else
  {
    const auto inputs = state.GetVertexInputs();
    std::cout << '[';
    for(size_t i = 0; i < inputs.size(); ++i)
    {
      if(i) std::cout << ',';
      const auto &input = inputs[i];
      std::cout << "{\"name\":"; json_string(input.name);
      std::cout << ",\"vertex_buffer\":" << input.vertexBuffer << ",\"byte_offset\":" << input.byteOffset
                << ",\"per_instance\":" << (input.perInstance ? "true" : "false")
                << ",\"instance_rate\":" << input.instanceRate << ",\"format\":";
      json_string(input.format.Name());
      std::cout << ",\"generic_enabled\":" << (input.genericEnabled ? "true" : "false")
                << ",\"used\":" << (input.used ? "true" : "false") << '}';
    }
    std::cout << ']';
  }
  size_t count = 0;
  if(state.IsCaptureD3D11()) count = controller->GetD3D11PipelineState()->rasterizer.viewports.size();
  else if(state.IsCaptureD3D12()) count = controller->GetD3D12PipelineState()->rasterizer.viewports.size();
  else if(state.IsCaptureGL()) count = controller->GetGLPipelineState()->rasterizer.viewports.size();
  else if(state.IsCaptureVK()) count = controller->GetVulkanPipelineState()->viewportScissor.viewportScissors.size();
  std::cout << ",\"viewports\":[";
  for(size_t i = 0; i < count; ++i)
  {
    if(i) std::cout << ',';
    const auto viewport = state.GetViewport(uint32_t(i));
    std::cout << "{\"enabled\":" << (viewport.enabled ? "true" : "false") << ",\"x\":"; number(viewport.x);
    std::cout << ",\"y\":"; number(viewport.y);
    std::cout << ",\"width\":"; number(viewport.width);
    std::cout << ",\"height\":"; number(viewport.height);
    std::cout << ",\"min_depth\":"; number(viewport.minDepth);
    std::cout << ",\"max_depth\":"; number(viewport.maxDepth);
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
  std::vector<TextureRef> texture_ids;
  std::vector<BufferRef> buffer_ids;
  std::map<uint32_t, uint32_t> event_chunks;
  write_snapshot(replay.controller, event_ids, texture_ids, buffer_ids, event_chunks);
  std::map<ResourceId, size_t> resource_indices;
  const auto &resources = replay.controller->GetResources();
  for(size_t i = 0; i < resources.size(); ++i) resource_indices.emplace(resources[i].resourceId, i);
  bool selected = false;
  uint32_t selected_event = 0;

  std::string command;
  while(std::getline(std::cin, command))
  {
    if(command == "quit") break;
    if(command.compare(0, 10, "structure ") == 0)
    {
      std::istringstream arguments(command.substr(10));
      std::string event_text, offset_text, limit_text, item;
      uint64_t event = 0, offset = 0, limit = 0;
      if(!(arguments >> event_text >> offset_text >> limit_text) || !decimal(event_text, event) ||
         !decimal(offset_text, offset) || !decimal(limit_text, limit) || event > UINT32_MAX || limit == 0 || limit > 128)
      { error("Invalid structured event request"); continue; }
      auto found = event_chunks.find(uint32_t(event));
      if(found == event_chunks.end()) { error("Structured data is unavailable for this event"); continue; }
      const SDObject *object = replay.controller->GetStructuredFile().chunks[found->second];
      std::vector<uint64_t> path;
      bool valid = true;
      while(arguments >> item)
      {
        uint64_t index = 0;
        if(!decimal(item, index) || index >= object->NumChildren()) { valid = false; break; }
        object = object->GetChild(size_t(index));
        if(!object) { valid = false; break; }
        path.push_back(index);
      }
      if(!valid || offset > object->NumChildren()) { error("Invalid structured data path or offset"); continue; }
      const size_t begin = size_t(offset);
      const size_t end = begin + std::min<size_t>(size_t(limit), object->NumChildren() - begin);
      std::vector<const SDObject *> children;
      for(size_t i = begin; i < end; ++i)
      {
        const SDObject *child = object->GetChild(i);
        if(!child) { valid = false; break; }
        children.push_back(child);
      }
      if(!valid) { error("Structured child is unavailable"); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"structure\",\"event_id\":" << event
                << ",\"offset\":\"" << offset << "\",\"path\":[";
      for(size_t i = 0; i < path.size(); ++i) { if(i) std::cout << ','; std::cout << '"' << path[i] << '"'; }
      std::cout << "],\"node\":"; write_structured_node(object, resource_indices);
      std::cout << ",\"children\":[";
      for(size_t i = 0; i < children.size(); ++i) { if(i) std::cout << ','; write_structured_node(children[i], resource_indices); }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 7, "shader ") == 0)
    {
      std::istringstream arguments(command.substr(7));
      std::string stage_text, target_text, extra;
      uint64_t stage_index = 0, target_index = 0;
      if(!(arguments >> stage_text >> target_text) || (arguments >> extra) || !decimal(stage_text, stage_index) ||
         stage_index >= 8 || (target_text != "default" && !decimal(target_text, target_index)))
      { error("Invalid shader stage or disassembly target"); continue; }
      if(!selected) { error("Select an event before inspecting shaders"); continue; }
      const PipeState &state = replay.controller->GetPipelineState();
      const ShaderStage stage = ShaderStage(stage_index);
      if(state.GetShader(stage) == ResourceId()) { error("No shader is bound to this stage"); continue; }
      const ShaderReflection *reflection = state.GetShaderReflection(stage);
      if(!reflection) { error("Shader reflection is unavailable"); continue; }
      const ResourceId pipeline = stage == ShaderStage::Compute ? state.GetComputePipelineObject() : state.GetGraphicsPipelineObject();
      const auto targets = replay.controller->GetDisassemblyTargets(pipeline != ResourceId());
      if(target_text != "default" && target_index >= targets.size()) { error("Invalid shader disassembly target"); continue; }
      const rdcstr target = target_text == "default" ? rdcstr() : targets[size_t(target_index)];
      const rdcstr disassembly = replay.controller->DisassembleShader(pipeline, reflection, target);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"shader\",\"event_id\":" << selected_event << ",\"stage\":" << stage_index << ",\"entry_point\":";
      json_string(reflection->entryPoint);
      std::cout << ",\"encoding\":" << uint32_t(reflection->encoding) << ",\"target\":"; json_string(target);
      std::cout << ",\"targets\":[";
      for(size_t i = 0; i < targets.size(); ++i) { if(i) std::cout << ','; json_string(targets[i]); }
      std::cout << "],\"disassembly\":"; json_string(disassembly);
      auto signature = [](const rdcarray<SigParameter> &parameters) {
        std::cout << '[';
        for(size_t i = 0; i < parameters.size(); ++i)
        {
          if(i) std::cout << ',';
          const auto &p = parameters[i];
          std::cout << "{\"name\":"; json_string(p.varName);
          std::cout << ",\"semantic\":"; json_string(p.semanticName);
          std::cout << ",\"semantic_index\":" << p.semanticIndex << ",\"register\":" << p.regIndex
                    << ",\"system_value\":" << uint32_t(p.systemValue) << ",\"type\":" << uint32_t(p.varType)
                    << ",\"components\":" << p.compCount << '}';
        }
        std::cout << ']';
      };
      std::cout << ",\"inputs\":"; signature(reflection->inputSignature);
      std::cout << ",\"outputs\":"; signature(reflection->outputSignature);
      std::cout << ",\"constant_blocks\":" << reflection->constantBlocks.size()
                << ",\"read_only_resources\":" << reflection->readOnlyResources.size()
                << ",\"read_write_resources\":" << reflection->readWriteResources.size()
                << ",\"samplers\":" << reflection->samplers.size() << "}" << std::endl;
      continue;
    }
    if(command.compare(0, 7, "buffer ") == 0)
    {
      std::istringstream arguments(command.substr(7));
      std::string index_text, offset_text, length_text, extra;
      uint64_t index = 0, offset = 0, length = 0;
      if(!(arguments >> index_text >> offset_text >> length_text) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(offset_text, offset) || !decimal(length_text, length) ||
         index >= buffer_ids.size() || length > 65536 || offset > buffer_ids[size_t(index)].length ||
         length > buffer_ids[size_t(index)].length - offset)
      { error("Invalid buffer range"); continue; }
      if(!selected) { error("Select an event before reading buffers"); continue; }
      bytebuf data;
      if(length) data = replay.controller->GetBufferData(buffer_ids[size_t(index)].id, offset, length);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(data.size() != length) { error("RenderDoc did not return the requested buffer range"); continue; }
      static const char hex[] = "0123456789abcdef";
      std::cout << "\nRENDERDOCX/1 {\"type\":\"buffer_data\",\"index\":" << index << ",\"offset\":\"" << offset << "\",\"hex\":\"";
      for(byte value : data) std::cout << hex[value >> 4] << hex[value & 15];
      std::cout << "\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 6, "save2 ") == 0)
    {
      if(!selected) { error("Select an event before saving textures"); continue; }
      std::istringstream arguments(command.substr(6));
      std::string index_text, mip_text, slice_text, sample_text, channel_text, encoded, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, channel = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> channel_text >> encoded) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) ||
         (channel_text != "all" && !decimal(channel_text, channel)) || index >= texture_ids.size())
      { error("Invalid texture selection"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      const uint64_t depth = texture.dimension == 3 ? (mip < 32 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : 1U) : texture.arraysize;
      if(mip >= texture.mips || mip > INT32_MAX || slice >= depth || slice > INT32_MAX ||
         (sample_text != "resolve" && sample >= texture.samples) ||
         (channel_text != "all" && (channel >= texture.components || channel > INT32_MAX)))
      { error("Texture selection is out of range"); continue; }
      std::string path;
      bool valid = !encoded.empty() && encoded.size() % 2 == 0;
      for(size_t i = 0; valid && i < encoded.size(); i += 2)
      {
        auto digit = [](char c) -> int { return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1; };
        int high = digit(encoded[i]), low = digit(encoded[i + 1]);
        if(high < 0 || low < 0 || (high == 0 && low == 0)) { valid = false; break; }
        path.push_back(char(high * 16 + low));
      }
      if(!valid) { error("Invalid output path"); continue; }
      TextureSave save;
      save.resourceId = texture.id;
      save.destType = FileType::PNG;
      save.mip = int32_t(mip);
      save.slice.sliceIndex = int32_t(slice);
      save.sample.sampleIndex = sample_text == "resolve" ? TextureSampleMapping::ResolveSamples : uint32_t(sample);
      save.channelExtract = channel_text == "all" ? -1 : int(channel);
      ResultDetails saved = replay.controller->SaveTexture(save, path.c_str());
      if(!saved.OK()) { error(result_message(saved)); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"texture_saved\",\"index\":" << index << "}" << std::endl;
      continue;
    }
    if(command.compare(0, 5, "save ") == 0)
    {
      if(!selected) { error("Select an event before saving textures"); continue; }
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
      save.resourceId = texture_ids[size_t(index)].id;
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
    if(!valid || event_ids.count(uint32_t(event)) == 0) { error("Unknown capture event"); continue; }
    replay.controller->SetFrameEvent(uint32_t(event), false);
    result = replay.controller->GetFatalErrorStatus();
    if(!result.OK()) { error(result_message(result)); break; }
    selected = true;
    selected_event = uint32_t(event);
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
