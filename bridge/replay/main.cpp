#include "api/replay/renderdoc_replay.h"
#include "api/replay/version.h"

#include <cstdint>
#include <algorithm>
#include <cstring>
#include <cwchar>
#include <cmath>
#include <filesystem>
#include <fstream>
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

uint32_t float_bits(float value) { uint32_t bits = 0; std::memcpy(&bits, &value, sizeof(bits)); return bits; }

void json_string(const rdcstr &value) { json_string(value.c_str(), value.size()); }
void write_reflected_constant(const ShaderConstant &value);
void write_reflected_type(const ShaderConstantType &value)
{
  std::cout << "{\"name\":"; json_string(value.name);
  std::cout << ",\"flags\":" << uint32_t(value.flags) << ",\"pointer_type_id\":" << value.pointerTypeID
            << ",\"elements\":" << value.elements << ",\"array_stride\":" << value.arrayByteStride
            << ",\"base_type\":" << uint32_t(value.baseType) << ",\"rows\":" << uint32_t(value.rows)
            << ",\"columns\":" << uint32_t(value.columns) << ",\"matrix_stride\":" << uint32_t(value.matrixByteStride)
            << ",\"members\":[";
  for(size_t i = 0; i < value.members.size(); ++i) { if(i) std::cout << ','; write_reflected_constant(value.members[i]); }
  std::cout << "]}";
}
void write_reflected_constant(const ShaderConstant &value)
{
  std::cout << "{\"name\":"; json_string(value.name);
  std::cout << ",\"byte_offset\":" << value.byteOffset << ",\"bit_offset\":" << value.bitFieldOffset
            << ",\"bit_size\":" << value.bitFieldSize << ",\"default_value\":\"" << value.defaultValue << "\",\"type\":";
  write_reflected_type(value.type); std::cout << '}';
}
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
  IRemoteServer *remote = nullptr;
  rdcstr remote_path;
  bool initialised = false;
  ~ReplayScope()
  {
    if(controller) { if(remote) remote->CloseCapture(controller); else controller->Shutdown(); }
    if(remote) remote->ShutdownConnection();
    if(file) file->Shutdown();
    if(initialised) RENDERDOC_ShutdownReplay();
  }
};

struct DebugTraceScope
{
  IReplayController *controller;
  ShaderDebugTrace *trace = nullptr;
  ~DebugTraceScope() { reset(); }
  void reset()
  {
    if(trace) controller->FreeTrace(trace);
    trace = nullptr;
  }
};

struct ScopedReplayOutput
{
  IReplayOutput *output = nullptr;
  ~ScopedReplayOutput() { if(output) output->Shutdown(); }
};

struct CustomShaderScope
{
  IReplayController *controller;
  ResourceId active;
  ~CustomShaderScope() { clear(); }
  void clear() { if(active != ResourceId()) controller->FreeCustomShader(active); active = ResourceId(); }
};

struct ShaderReplacementScope
{
  IReplayController *controller;
  std::map<ResourceId, ResourceId> replacements;
  ~ShaderReplacementScope()
  {
    for(const auto &entry : replacements)
    {
      controller->RemoveReplacement(entry.first);
      controller->FreeTargetResource(entry.second);
    }
  }
  ResourceId original(ResourceId id) const
  {
    for(const auto &entry : replacements) if(entry.second == id) return entry.first;
    return id;
  }
  void remove(ResourceId id)
  {
    auto it = replacements.find(id);
    if(it == replacements.end()) return;
    controller->RemoveReplacement(id);
    controller->FreeTargetResource(it->second);
    replacements.erase(it);
  }
};

void write_debug_variable(const ShaderVariable &variable)
{
  std::cout << "{\"name\":"; json_string(variable.name);
  std::cout << ",\"rows\":" << unsigned(variable.rows) << ",\"columns\":" << unsigned(variable.columns)
            << ",\"type\":" << uint32_t(variable.type) << ",\"flags\":" << uint32_t(variable.flags)
            << ",\"value_hex\":\"";
  const auto *bytes = reinterpret_cast<const unsigned char *>(&variable.value);
  static const char hex[] = "0123456789abcdef";
  for(size_t i = 0; i < sizeof(variable.value); ++i)
    std::cout << hex[bytes[i] >> 4] << hex[bytes[i] & 15];
  std::cout << "\",\"members\":[";
  for(size_t i = 0; i < variable.members.size(); ++i)
  {
    if(i) std::cout << ',';
    write_debug_variable(variable.members[i]);
  }
  std::cout << "]}";
}

void write_debug_variables(const rdcarray<ShaderVariable> &variables)
{
  std::cout << '[';
  for(size_t i = 0; i < variables.size(); ++i)
  {
    if(i) std::cout << ',';
    write_debug_variable(variables[i]);
  }
  std::cout << ']';
}

void write_source_variables(const rdcarray<SourceVariableMapping> &variables)
{
  std::cout << '[';
  for(size_t i = 0; i < variables.size(); ++i)
  {
    if(i) std::cout << ',';
    const auto &variable = variables[i];
    std::cout << "{\"name\":"; json_string(variable.name);
    std::cout << ",\"type\":" << uint32_t(variable.type) << ",\"undefined\":" << (variable.undefinedValue ? "true" : "false")
              << ",\"rows\":" << variable.rows << ",\"columns\":" << variable.columns << ",\"offset\":" << variable.offset
              << ",\"signature_index\":" << variable.signatureIndex << ",\"variables\":[";
    for(size_t j = 0; j < variable.variables.size(); ++j)
    {
      if(j) std::cout << ',';
      const auto &reference = variable.variables[j];
      std::cout << "{\"name\":"; json_string(reference.name);
      std::cout << ",\"type\":" << uint32_t(reference.type) << ",\"component\":" << reference.component << '}';
    }
    std::cout << "]}";
  }
  std::cout << ']';
}

void write_debug_trace(uint32_t event, const ShaderDebugTrace &trace)
{
  std::cout << "\nRENDERDOCX/1 {\"type\":\"debug_started\",\"event_id\":" << event
            << ",\"stage\":" << uint32_t(trace.stage) << ",\"inputs\":";
  write_debug_variables(trace.inputs);
  std::cout << ",\"constant_blocks\":"; write_debug_variables(trace.constantBlocks);
  std::cout << ",\"read_only_resources\":"; write_debug_variables(trace.readOnlyResources);
  std::cout << ",\"read_write_resources\":"; write_debug_variables(trace.readWriteResources);
  std::cout << ",\"samplers\":"; write_debug_variables(trace.samplers);
  std::cout << ",\"source_variables\":"; write_source_variables(trace.sourceVars);
  std::cout << ",\"instructions\":[";
  for(size_t i = 0; i < trace.instInfo.size(); ++i)
  {
    if(i) std::cout << ',';
    const auto &info = trace.instInfo[i];
    const auto &line = info.lineInfo;
    std::cout << "{\"instruction\":" << info.instruction << ",\"disassembly_line\":" << line.disassemblyLine
              << ",\"file_index\":" << line.fileIndex << ",\"line_start\":" << line.lineStart
              << ",\"line_end\":" << line.lineEnd << ",\"column_start\":" << line.colStart
              << ",\"column_end\":" << line.colEnd << ",\"source_variables\":";
    write_source_variables(info.sourceVars);
    std::cout << '}';
  }
  std::cout << ']';
  std::cout << "}" << std::endl;
}

struct PendingAction
{
  const ActionDescription *action;
  size_t depth;
  int64_t parent;
};

struct BufferRef { ResourceId id; uint64_t length; };
struct TextureRef { ResourceId id; uint32_t width, height, dimension, depth, mips, arraysize, samples, components; };

void write_debug_message(const DebugMessage &message)
{
  std::cout << "{\"event_id\":" << message.eventId << ",\"category\":" << uint32_t(message.category)
            << ",\"severity\":" << uint32_t(message.severity) << ",\"source\":" << uint32_t(message.source)
            << ",\"message_id\":" << message.messageID << ",\"description\":";
  json_string(message.description);
  std::cout << '}';
}

void write_descriptor(const Descriptor &d, const std::map<ResourceId, size_t> &indices)
{
  auto resource = [&](ResourceId id) {
    const auto it = indices.find(id);
    if(id == ResourceId() || it == indices.end()) std::cout << "null"; else std::cout << it->second;
  };
  uint32_t lod; std::memcpy(&lod, &d.minLODClamp, sizeof(lod));
  std::cout << "{\"type\":" << uint32_t(d.type) << ",\"flags\":" << uint32_t(d.flags) << ",\"format\":";
  json_string(d.format.Name());
  std::cout << ",\"format_component_type\":" << uint32_t(d.format.compType);
  std::cout << ",\"resource\":"; resource(d.resource);
  std::cout << ",\"resource_present\":" << (d.resource != ResourceId() ? "true" : "false");
  std::cout << ",\"secondary\":"; resource(d.secondary);
  std::cout << ",\"secondary_present\":" << (d.secondary != ResourceId() ? "true" : "false");
  std::cout << ",\"view\":"; resource(d.view);
  std::cout << ",\"view_present\":" << (d.view != ResourceId() ? "true" : "false");
  std::cout << ",\"byte_offset\":\"" << d.byteOffset << "\",\"byte_size\":\"" << d.byteSize
            << "\",\"counter_byte_offset\":" << d.counterByteOffset << ",\"buffer_struct_count\":" << d.bufferStructCount
            << ",\"element_byte_size\":" << d.elementByteSize << ",\"min_lod_bits\":" << lod
            << ",\"first_slice\":" << d.firstSlice << ",\"num_slices\":" << d.numSlices
            << ",\"first_mip\":" << uint32_t(d.firstMip) << ",\"num_mips\":" << uint32_t(d.numMips)
            << ",\"texture_type\":" << uint32_t(d.textureType) << ",\"swizzle\":[" << uint32_t(d.swizzle.red) << ','
            << uint32_t(d.swizzle.green) << ',' << uint32_t(d.swizzle.blue) << ',' << uint32_t(d.swizzle.alpha) << "]}";
}

void write_sampler(const SamplerDescriptor &s, const std::map<ResourceId, size_t> &indices)
{
  auto bits = [](float v) { uint32_t raw; std::memcpy(&raw, &v, sizeof(raw)); return raw; };
  uint32_t border[4]; std::memcpy(border, &s.borderColorValue, sizeof(border));
  const auto object = indices.find(s.object), ycbcr = indices.find(s.ycbcrSampler);
  std::cout << "{\"object\":";
  if(s.object == ResourceId() || object == indices.end()) std::cout << "null"; else std::cout << object->second;
  std::cout << ",\"type\":" << uint32_t(s.type) << ",\"address\":[" << uint32_t(s.addressU) << ',' << uint32_t(s.addressV) << ',' << uint32_t(s.addressW)
            << "],\"compare\":" << uint32_t(s.compareFunction) << ",\"filter\":[" << uint32_t(s.filter.minify) << ',' << uint32_t(s.filter.magnify)
            << ',' << uint32_t(s.filter.mip) << ',' << uint32_t(s.filter.filter) << "],\"srgb_border\":" << (s.srgbBorder ? "true" : "false")
            << ",\"seamless_cubemaps\":" << (s.seamlessCubemaps ? "true" : "false") << ",\"unnormalized\":" << (s.unnormalized ? "true" : "false")
            << ",\"max_anisotropy_bits\":" << bits(s.maxAnisotropy) << ",\"max_lod_bits\":" << bits(s.maxLOD)
            << ",\"min_lod_bits\":" << bits(s.minLOD) << ",\"mip_bias_bits\":" << bits(s.mipBias)
            << ",\"border_type\":" << uint32_t(s.borderColorType) << ",\"border_bits\":[" << border[0] << ',' << border[1] << ',' << border[2] << ',' << border[3]
            << "],\"swizzle\":[" << uint32_t(s.swizzle.red) << ',' << uint32_t(s.swizzle.green) << ',' << uint32_t(s.swizzle.blue) << ',' << uint32_t(s.swizzle.alpha)
            << "],\"ycbcr\":";
  if(s.ycbcrSampler == ResourceId()) std::cout << "null";
  else std::cout << "{\"model\":" << uint32_t(s.ycbcrModel) << ",\"range\":" << uint32_t(s.ycbcrRange)
                 << ",\"chroma_offsets\":[" << uint32_t(s.xChromaOffset) << ',' << uint32_t(s.yChromaOffset)
                 << "],\"chroma_filter\":" << uint32_t(s.chromaFilter) << ",\"force_explicit_reconstruction\":"
                 << (s.forceExplicitReconstruction ? "true" : "false") << '}';
  std::cout << ",\"creation_time_constant\":" << (s.creationTimeConstant ? "true" : "false") << ",\"ycbcr_sampler\":";
  if(s.ycbcrSampler == ResourceId() || ycbcr == indices.end()) std::cout << "null"; else std::cout << ycbcr->second;
  std::cout << '}';
}

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

bool decode_section_hex(const std::string &text, bytebuf &bytes)
{
  if(text == "_") return true;
  if(text.empty() || text.size() % 2 != 0) return false;
  auto digit = [](char c) -> int { if(c >= '0' && c <= '9') return c - '0'; if(c >= 'a' && c <= 'f') return c - 'a' + 10; return -1; };
  for(size_t i = 0; i < text.size(); i += 2)
  { int a = digit(text[i]), b = digit(text[i + 1]); if(a < 0 || b < 0) return false; bytes.push_back(byte((a << 4) | b)); }
  return true;
}

void write_section_hex(const bytebuf &bytes)
{
  static const char hex[] = "0123456789abcdef";
  for(byte b : bytes) std::cout << hex[b >> 4] << hex[b & 15];
}

void write_snapshot(IReplayController *controller, const ReplayOptions &options, std::set<uint32_t> &event_ids,
                    std::vector<TextureRef> &texture_ids, std::vector<BufferRef> &buffer_ids,
                    std::map<uint32_t, uint32_t> &event_chunks,
                    std::map<uint32_t, const SDObject *> &event_annotations,
                    std::map<uint32_t, uint32_t> &draw_instances,
                    std::map<uint32_t, uint32_t> &draw_vertices,
                    std::set<uint32_t> &indexed_draws,
                    std::map<uint32_t, const ActionDescription *> &dispatch_actions)
{
  const APIProperties api = controller->GetAPIProperties();
  const FrameDescription frame = controller->GetFrameInfo();
  const rdcarray<DebugMessage> replay_messages = controller->GetDebugMessages();
  const SDFile &structured = controller->GetStructuredFile();
  const auto &roots = controller->GetRootActions();
  std::vector<PendingAction> pending;
  for(size_t i = roots.size(); i > 0; --i) pending.push_back({&roots[i - 1], 0, -1});

  std::cout << "\nRENDERDOCX/1 {\"type\":\"snapshot\",\"protocol\":1,\"api\":" << uint32_t(api.pipelineType)
            << ",\"local_renderer\":" << uint32_t(api.localRenderer)
            << ",\"frame\":" << frame.frameNumber << ",\"degraded\":"
            << (api.degraded ? "true" : "false")
            << ",\"replay_options\":{\"api_validation\":" << (options.apiValidation ? "true" : "false")
            << ",\"gpu_vendor\":" << uint32_t(options.forceGPUVendor) << ",\"gpu_device_id\":" << options.forceGPUDeviceID
            << ",\"optimisation\":" << uint32_t(options.optimisation) << ",\"gpu_driver\":";
  json_string(options.forceGPUDriverName);
  std::cout << "},\"frame_info\":{\"file_offset\":\"" << frame.fileOffset
            << "\",\"uncompressed_size\":\"" << frame.uncompressedFileSize
            << "\",\"compressed_size\":\"" << frame.compressedFileSize
            << "\",\"persistent_size\":\"" << frame.persistentSize
            << "\",\"initial_data_size\":\"" << frame.initDataSize
            << "\",\"capture_time\":\"" << frame.captureTime
            << "\",\"contains_annotations\":" << (frame.containsAnnotations ? "true" : "false") << "}"
            << ",\"debug_messages\":[";
  bool first_message = true;
  auto write_messages = [&](const rdcarray<DebugMessage> &messages) {
    for(const DebugMessage &message : messages)
    {
      if(!first_message) std::cout << ',';
      first_message = false;
      write_debug_message(message);
    }
  };
  write_messages(frame.debugMessages);
  write_messages(replay_messages);
  std::cout << "],\"frame_statistics\":{\"recorded\":" << (frame.stats.recorded ? "true" : "false");
  if(frame.stats.recorded)
  {
    const FrameStatistics &stats = frame.stats;
    auto array = [](const rdcarray<uint32_t> &values) {
      std::cout << '[';
      for(size_t i = 0; i < values.size(); ++i) { if(i) std::cout << ','; std::cout << values[i]; }
      std::cout << ']';
    };
    auto bind = [](const auto &value) {
      std::cout << "{\"calls\":" << value.calls << ",\"sets\":" << value.sets << ",\"nulls\":" << value.nulls;
    };
    auto simple_bind = [&](const auto &value) { bind(value); std::cout << '}'; };
    auto redundant_bind = [&](const auto &value) { bind(value); std::cout << ",\"redundants\":" << value.redundants << '}'; };
    std::cout << ",\"draws\":{\"calls\":" << stats.draws.calls << ",\"instanced\":" << stats.draws.instanced
              << ",\"indirect\":" << stats.draws.indirect << ",\"counts\":";
    array(stats.draws.counts);
    std::cout << "},\"dispatches\":{\"calls\":" << stats.dispatches.calls << ",\"indirect\":" << stats.dispatches.indirect << '}';
    std::cout << ",\"indices\":"; simple_bind(stats.indices);
    std::cout << ",\"layouts\":"; simple_bind(stats.layouts);
    std::cout << ",\"vertices\":"; bind(stats.vertices); std::cout << ",\"bindslots\":"; array(stats.vertices.bindslots); std::cout << '}';
    std::cout << ",\"updates\":{\"calls\":" << stats.updates.calls << ",\"clients\":" << stats.updates.clients << ",\"servers\":" << stats.updates.servers << ",\"types\":";
    array(stats.updates.types); std::cout << ",\"sizes\":"; array(stats.updates.sizes); std::cout << '}';
    std::cout << ",\"constants\":[";
    for(size_t i = 0; i < stats.constants.size(); ++i) { if(i) std::cout << ','; const auto &v=stats.constants[i]; bind(v); std::cout << ",\"bindslots\":"; array(v.bindslots); std::cout << ",\"sizes\":"; array(v.sizes); std::cout << '}'; }
    std::cout << "],\"samplers\":[";
    for(size_t i = 0; i < stats.samplers.size(); ++i) { if(i) std::cout << ','; const auto &v=stats.samplers[i]; bind(v); std::cout << ",\"bindslots\":"; array(v.bindslots); std::cout << '}'; }
    std::cout << "],\"resources\":[";
    for(size_t i = 0; i < stats.resources.size(); ++i) { if(i) std::cout << ','; const auto &v=stats.resources[i]; bind(v); std::cout << ",\"types\":"; array(v.types); std::cout << ",\"bindslots\":"; array(v.bindslots); std::cout << '}'; }
    std::cout << "],\"shaders\":[";
    for(size_t i = 0; i < stats.shaders.size(); ++i) { if(i) std::cout << ','; redundant_bind(stats.shaders[i]); }
    std::cout << "],\"blends\":"; redundant_bind(stats.blends);
    std::cout << ",\"depths\":"; redundant_bind(stats.depths);
    std::cout << ",\"rasters\":"; bind(stats.rasters); std::cout << ",\"redundants\":" << stats.rasters.redundants << ",\"viewports\":"; array(stats.rasters.viewports); std::cout << ",\"rects\":"; array(stats.rasters.rects); std::cout << '}';
    std::cout << ",\"outputs\":"; bind(stats.outputs); std::cout << ",\"bindslots\":"; array(stats.outputs.bindslots); std::cout << '}';
  }
  std::cout << "},\"counters\":[";
  const rdcarray<GPUCounter> available_counters = controller->EnumerateCounters();
  for(size_t i = 0; i < available_counters.size(); ++i)
  {
    if(i) std::cout << ',';
    const CounterDescription counter = controller->DescribeCounter(available_counters[i]);
    std::cout << "{\"id\":" << uint32_t(counter.counter) << ",\"name\":"; json_string(counter.name);
    std::cout << ",\"category\":"; json_string(counter.category);
    std::cout << ",\"description\":"; json_string(counter.description);
    std::cout << ",\"result_type\":" << uint32_t(counter.resultType) << ",\"result_byte_width\":" << counter.resultByteWidth
              << ",\"unit\":" << uint32_t(counter.unit) << ",\"uuid\":[" << counter.uuid.words[0] << ',' << counter.uuid.words[1] << ',' << counter.uuid.words[2] << ',' << counter.uuid.words[3] << "]}";
  }
  std::cout << "],\"actions\":[";
  size_t index = 0;
  while(!pending.empty())
  {
    const PendingAction node = pending.back();
    pending.pop_back();
    const ActionDescription &action = *node.action;
    if(index) std::cout << ',';
    std::cout << "{\"event_id\":" << action.eventId << ",\"action_id\":" << action.actionId
              << ",\"depth\":" << node.depth << ",\"parent\":" << node.parent
              << ",\"flags\":" << uint32_t(action.flags) << ",\"marker_color_bits\":[" << float_bits(action.markerColor.x) << ',' << float_bits(action.markerColor.y) << ',' << float_bits(action.markerColor.z) << ',' << float_bits(action.markerColor.w) << "],\"marker_name\":";
    json_string(action.customName);
    std::cout << ",\"indices\":" << action.numIndices
              << ",\"instances\":" << action.numInstances
              << ",\"base_vertex\":" << action.baseVertex << ",\"index_offset\":" << action.indexOffset
              << ",\"vertex_offset\":" << action.vertexOffset << ",\"instance_offset\":" << action.instanceOffset
              << ",\"dispatch_groups\":[" << action.dispatchDimension[0] << ',' << action.dispatchDimension[1] << ',' << action.dispatchDimension[2] << ']'
              << ",\"dispatch_threads\":[" << action.dispatchThreadsDimension[0] << ',' << action.dispatchThreadsDimension[1] << ',' << action.dispatchThreadsDimension[2] << ']'
              << ",\"name\":";
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
      event_annotations.emplace(event.eventId, event.annotations);
      if(event.chunkIndex < structured.chunks.size() && structured.chunks[event.chunkIndex])
        event_chunks.emplace(event.eventId, event.chunkIndex);
    }
    std::cout << "]}";
    event_ids.insert(action.eventId);
    if(action.flags & (ActionFlags::Drawcall | ActionFlags::MeshDispatch))
      draw_instances.emplace(action.eventId, (std::max)(1U, action.numInstances));
    if(action.flags & ActionFlags::Drawcall)
      draw_vertices.emplace(action.eventId, action.numIndices);
    if((action.flags & ActionFlags::Drawcall) && (action.flags & ActionFlags::Indexed))
      indexed_draws.insert(action.eventId);
    if(action.flags & (ActionFlags::Dispatch | ActionFlags::MeshDispatch))
      dispatch_actions.emplace(action.eventId, &action);
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
            << textures.size() << ",\"buffer_count\":" << buffers.size() << ",\"resources\":[";
  auto resource_links = [&](const rdcarray<ResourceId> &ids) {
    std::cout << '[';
    for(size_t j = 0; j < ids.size(); ++j)
    {
      if(j) std::cout << ',';
      auto found = resource_indices.find(ids[j]);
      if(found == resource_indices.end() || ids[j] == ResourceId()) std::cout << "null";
      else std::cout << found->second;
    }
    std::cout << ']';
  };
  for(size_t i = 0; i < resources.size(); ++i)
  {
    if(i) std::cout << ',';
    const ResourceDescription &resource = resources[i];
    std::cout << "{\"name\":"; json_string(resource.name);
    static_assert(sizeof(ResourceId) == sizeof(uint64_t), "ResourceId encoding must match RenderDoc");
    uint64_t rename_id = 0; std::memcpy(&rename_id, &resource.resourceId, sizeof(rename_id));
    std::cout << ",\"rename_key\":\"ResourceId::" << rename_id << "\"";
    std::cout << ",\"type\":" << uint32_t(resource.type) << ",\"autogenerated_name\":" << (resource.autogeneratedName ? "true" : "false")
              << ",\"initialisation_chunks\":[";
    for(size_t j = 0; j < resource.initialisationChunks.size(); ++j)
    { if(j) std::cout << ','; std::cout << resource.initialisationChunks[j]; }
    std::cout << "],\"parents\":"; resource_links(resource.parentResources);
    std::cout << ",\"derived\":"; resource_links(resource.derivedResources);
    std::cout << '}';
  }
  std::cout << "],\"descriptor_stores\":[";
  const auto &stores = controller->GetDescriptorStores();
  for(size_t i = 0; i < stores.size(); ++i)
  {
    if(i) std::cout << ',';
    const auto &store = stores[i];
    const auto resource = resource_indices.find(store.resourceId);
    std::cout << "{\"resource\":";
    if(resource == resource_indices.end()) std::cout << "null"; else std::cout << resource->second;
    std::cout << ",\"descriptor_byte_size\":" << store.descriptorByteSize << ",\"first_offset\":" << store.firstDescriptorOffset
              << ",\"count\":" << store.descriptorCount << '}';
  }
  std::cout << "],\"textures\":[";
  for(size_t i = 0; i < textures.size(); ++i)
  {
    const TextureDescription &texture = textures[i];
    texture_ids.push_back({texture.resourceId, texture.width, texture.height, texture.dimension, texture.depth, texture.mips, texture.arraysize, texture.msSamp, texture.format.compCount});
    const auto found_name = resource_names.find(texture.resourceId);
    const auto resource_index = resource_indices.find(texture.resourceId);
    const rdcstr *name = found_name == resource_names.end() ? nullptr : found_name->second;
    if(i) std::cout << ',';
    std::cout << "{\"name\":";
    if(name) json_string(*name); else json_string("", 0);
    std::cout << ",\"resource_index\":";
    if(resource_index == resource_indices.end()) std::cout << "null"; else std::cout << resource_index->second;
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

void write_mesh(const MeshFormat &mesh, uint32_t event, uint32_t instance, uint32_t view, uint32_t stage, bool restart_enabled, uint32_t restart_index, const ShaderReflection *task_shader, bool vulkan_task_layout, const PipeState &pipeline)
{
  auto number = [](float value) {
    if(std::isfinite(value)) std::cout << std::setprecision(std::numeric_limits<float>::max_digits10) << value;
    else std::cout << "null";
  };
  std::cout << "\nRENDERDOCX/1 {\"type\":\"mesh\",\"event_id\":" << event << ",\"instance\":" << instance
            << ",\"view\":" << view << ",\"stage\":" << stage
            << ",\"index_bound\":" << (mesh.indexResourceId != ResourceId() ? "true" : "false")
            << ",\"index_offset\":\"" << mesh.indexByteOffset << "\",\"index_size\":\"" << mesh.indexByteSize
            << "\",\"index_stride\":" << mesh.indexByteStride << ",\"base_vertex\":" << mesh.baseVertex
            << ",\"vertex_bound\":" << (mesh.vertexResourceId != ResourceId() ? "true" : "false")
            << ",\"vertex_offset\":\"" << mesh.vertexByteOffset << "\",\"vertex_size\":\"" << mesh.vertexByteSize
            << "\",\"vertex_stride\":" << mesh.vertexByteStride << ",\"per_primitive_offset\":\"" << mesh.perPrimitiveOffset
            << "\",\"per_primitive_stride\":" << mesh.perPrimitiveStride << ",\"format_name\":";
  json_string(mesh.format.Name());
  std::cout << ",\"format_type\":" << uint32_t(mesh.format.type) << ",\"component_type\":" << uint32_t(mesh.format.compType)
            << ",\"component_count\":" << uint32_t(mesh.format.compCount) << ",\"component_byte_width\":" << uint32_t(mesh.format.compByteWidth)
            << ",\"topology\":" << uint32_t(mesh.topology) << ",\"num_indices\":" << mesh.numIndices
            << ",\"restart_index\":" << mesh.restartIndex << ",\"allow_restart\":" << (mesh.allowRestart ? "true" : "false")
            << ",\"index_restart\":";
  if(restart_enabled && restart_index != 0) std::cout << restart_index; else std::cout << "null";
  std::cout
            << ",\"unproject\":" << (mesh.unproject ? "true" : "false") << ",\"flip_y\":" << (mesh.flipY ? "true" : "false")
            << ",\"near_plane\":"; number(mesh.nearPlane); std::cout << ",\"far_plane\":"; number(mesh.farPlane);
  std::cout << ",\"meshlet_offset\":" << mesh.meshletOffset << ",\"meshlet_index_offset\":" << mesh.meshletIndexOffset
            << ",\"dispatch_size\":";
  // Upstream only defines dispatchSize for an available task-stage output.
  if(stage == uint32_t(MeshDataStage::TaskOut) && mesh.vertexResourceId != ResourceId())
    std::cout << '[' << mesh.dispatchSize[0] << ',' << mesh.dispatchSize[1] << ',' << mesh.dispatchSize[2] << ']';
  else std::cout << "null";
  std::cout << ",\"meshlets\":[";
  for(size_t i = 0; i < mesh.meshletSizes.size(); ++i)
  {
    if(i) std::cout << ',';
    std::cout << "{\"indices\":" << mesh.meshletSizes[i].numIndices << ",\"vertices\":" << mesh.meshletSizes[i].numVertices << '}';
  }
  std::cout << "],\"tasks\":[";
  for(size_t i = 0; i < mesh.taskSizes.size(); ++i)
  {
    if(i) std::cout << ',';
    std::cout << '[' << mesh.taskSizes[i].x << ',' << mesh.taskSizes[i].y << ',' << mesh.taskSizes[i].z << ']';
  }
  std::cout << "],\"task_payload_scalar_layout\":" << (vulkan_task_layout ? "true" : "false") << ",\"task_payload\":[";
  if(stage == uint32_t(MeshDataStage::TaskOut) && task_shader)
    for(size_t i = 0; i < task_shader->taskPayload.variables.size(); ++i)
    { if(i) std::cout << ','; write_reflected_constant(task_shader->taskPayload.variables[i]); }
  std::cout << "],\"primitive_vertices\":" << RENDERDOC_NumVerticesPerPrimitive(mesh.topology) << ",\"attributes\":[";
  const ShaderReflection *shader = nullptr;
  int32_t stream = 0;
  if(stage == uint32_t(MeshDataStage::VSOut)) shader = pipeline.GetShaderReflection(ShaderStage::Vertex);
  else if(stage == uint32_t(MeshDataStage::GSOut))
  {
    shader = pipeline.GetShaderReflection(ShaderStage::Geometry);
    if(!shader) shader = pipeline.GetShaderReflection(ShaderStage::Domain);
    stream = pipeline.GetRasterizedStream();
  }
  else if(stage == uint32_t(MeshDataStage::MeshOut)) shader = pipeline.GetShaderReflection(ShaderStage::Mesh);
  std::vector<const SigParameter *> columns;
  if(shader)
    for(const SigParameter &sig : shader->outputSignature)
      if(sig.stream == uint32_t(stream) && sig.systemValue != ShaderBuiltin::OutputIndices)
        columns.push_back(&sig);
  for(size_t i = 0; i < columns.size(); ++i)
    if(columns[i]->systemValue == ShaderBuiltin::Position)
    { std::rotate(columns.begin(), columns.begin() + i, columns.begin() + i + 1); break; }
  uint64_t vertex_offset = 0, primitive_offset = 0;
  const bool aligned = pipeline.HasAlignedPostVSData(MeshDataStage(stage));
  for(size_t i = 0; i < columns.size(); ++i)
  {
    const SigParameter &sig = *columns[i];
    uint64_t &offset = sig.perPrimitiveRate ? primitive_offset : vertex_offset;
    const uint32_t width = (std::max)(4U, VarTypeByteSize(sig.varType));
    const uint32_t alignment = aligned ? (sig.compCount > 2 ? 4U : sig.compCount == 2 ? 2U : 1U) * width : 1U;
    offset = (offset + alignment - 1) / alignment * alignment;
    if(i) std::cout << ',';
    std::cout << "{\"name\":"; json_string(sig.varName.empty() ? sig.semanticIdxName : sig.varName);
    std::cout << ",\"base_type\":" << uint32_t(sig.varType) << ",\"components\":" << sig.compCount
              << ",\"component_width\":" << width << ",\"byte_offset\":\"" << offset
              << "\",\"per_primitive\":" << (sig.perPrimitiveRate ? "true" : "false") << '}';
    offset += uint64_t(sig.compCount) * width;
  }
  std::cout << "],\"status\":"; json_string(mesh.status); std::cout << '}' << std::endl;
}

void write_modification_value(const ModificationValue &value)
{
  if(!value.IsValid()) { std::cout << "null"; return; }
  uint32_t bits[4], depth_bits = 0;
  std::memcpy(bits, &value.col, sizeof(bits));
  std::memcpy(&depth_bits, &value.depth, sizeof(depth_bits));
  std::cout << "{\"color_bits\":[" << bits[0] << ',' << bits[1] << ',' << bits[2] << ',' << bits[3]
            << "],\"depth_bits\":" << depth_bits << ",\"stencil\":" << value.stencil << '}';
}

void write_pixel_modification(const PixelModification &mod)
{
  std::cout << "{\"event_id\":" << mod.eventId << ",\"fragment\":" << mod.fragIndex << ",\"primitive\":" << mod.primitiveID
            << ",\"direct_shader_write\":" << (mod.directShaderWrite ? "true" : "false")
            << ",\"unbound_pixel_shader\":" << (mod.unboundPS ? "true" : "false")
            << ",\"passed\":" << (mod.Passed() ? "true" : "false") << ",\"pre\":";
  write_modification_value(mod.preMod); std::cout << ",\"shader_output\":"; write_modification_value(mod.shaderOut);
  std::cout << ",\"post\":"; write_modification_value(mod.postMod);
  std::cout << ",\"tests\":{\"sample_masked\":" << (mod.sampleMasked ? "true" : "false")
            << ",\"backface_culled\":" << (mod.backfaceCulled ? "true" : "false")
            << ",\"depth_clipped\":" << (mod.depthClipped ? "true" : "false")
            << ",\"depth_bounds_failed\":" << (mod.depthBoundsFailed ? "true" : "false")
            << ",\"view_clipped\":" << (mod.viewClipped ? "true" : "false")
            << ",\"scissor_clipped\":" << (mod.scissorClipped ? "true" : "false")
            << ",\"shader_discarded\":" << (mod.shaderDiscarded ? "true" : "false")
            << ",\"depth_test_failed\":" << (mod.depthTestFailed ? "true" : "false")
            << ",\"stencil_test_failed\":" << (mod.stencilTestFailed ? "true" : "false")
            << ",\"predication_skipped\":" << (mod.predicationSkipped ? "true" : "false") << "}}";
}

// Preserve upstream type metadata and scalar values; child arrays are paged separately.
void write_structured_node(const SDObject *object, const std::map<ResourceId, size_t> &resources,
                           const ShaderReplacementScope *replacements = nullptr)
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
  std::cout << ",\"resource_id\":";
  if(object->type.basetype == SDBasic::Resource)
  {
    uint64_t id = 0; const ResourceId resource = object->AsResourceId();
    std::memcpy(&id, &resource, sizeof(id));
    std::cout << '"' << id << '"';
  }
  else std::cout << "null";
  std::cout << ",\"resource_edited\":";
  if(object->type.basetype == SDBasic::Resource && replacements)
    std::cout << (replacements->replacements.count(object->AsResourceId()) ? "true" : "false");
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
  auto attachment = [&](const Descriptor &value) {
    std::cout << "{\"resource\":{"; resource(value.resource);
    std::cout << "},\"view\":{"; resource(value.view);
    std::cout << "},\"type\":" << uint32_t(value.type) << ",\"flags\":" << uint32_t(value.flags) << ",\"format\":";
    json_string(value.format.Name());
    std::cout << ",\"format_component_type\":" << uint32_t(value.format.compType);
    std::cout << ",\"texture_type\":" << uint32_t(value.textureType)
              << ",\"first_mip\":" << uint32_t(value.firstMip) << ",\"mip_count\":" << uint32_t(value.numMips)
              << ",\"first_slice\":" << uint32_t(value.firstSlice) << ",\"slice_count\":" << uint32_t(value.numSlices)
              << ",\"min_lod_clamp\":"; number(value.minLODClamp);
    std::cout << ",\"swizzle\":[" << uint32_t(value.swizzle.red) << ',' << uint32_t(value.swizzle.green) << ','
              << uint32_t(value.swizzle.blue) << ',' << uint32_t(value.swizzle.alpha) << "]}";
  };
  std::cout << "{\"event_id\":" << event << ",\"available\":" << (state.IsCaptureLoaded() ? "true" : "false")
            << ",\"topology\":" << uint32_t(state.GetPrimitiveTopology())
            << ",\"multiview_count\":" << state.MultiviewBroadcastCount() << ",\"shaders\":[";
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
  std::cout << "],\"color_attachments\":[";
  for(size_t i = 0; i < outputs.size(); ++i) { if(i) std::cout << ','; attachment(outputs[i]); }
  std::cout << "],\"depth_attachment\":"; attachment(state.GetDepthTarget());
  std::cout << ",\"depth_resolve_attachment\":"; attachment(state.GetDepthResolveTarget());
  std::cout << ",\"index_buffer\":";
  buffer(state.GetIBuffer());
  std::cout << ",\"restart_enabled\":" << (state.IsRestartEnabled() ? "true" : "false")
            << ",\"restart_index\":" << state.GetRestartIndex();
  std::cout << ",\"vertex_buffers\":[";
  const auto vertex_buffers = state.GetVBuffers();
  for(size_t i = 0; i < vertex_buffers.size(); ++i)
  {
    if(i) std::cout << ',';
    buffer(vertex_buffers[i]);
  }
  std::cout << "],\"vertex_inputs\":";
  if(!state.IsCaptureLoaded() || (state.IsCaptureGL() &&
      (!controller->GetGLPipelineState() || !controller->GetGLPipelineState()->vertexShader.reflection))) std::cout << "null";
  else
  {
    rdcarray<VertexInputAttribute> inputs;
    if(state.IsCaptureGL())
    {
      // GetVertexInputs() allocates for all GL attributes but only initializes
      // active entries. Construct only the entries backed by shader reflection.
      const GLPipe::State *gl = controller->GetGLPipelineState();
      if(gl && gl->vertexShader.reflection)
      {
        const auto &signature = gl->vertexShader.reflection->inputSignature;
        const auto &attributes = gl->vertexInput.attributes;
        const auto &bindings = gl->vertexInput.vertexBuffers;
        for(const GLPipe::VertexAttribute &attr : attributes)
        {
          if(attr.boundShaderInput < 0 || size_t(attr.boundShaderInput) >= signature.size()) continue;
          const SigParameter &sig = signature[size_t(attr.boundShaderInput)];
          VertexInputAttribute input;
          input.name = sig.varName;
          input.vertexBuffer = attr.vertexBufferSlot <= uint32_t(INT32_MAX) ? int(attr.vertexBufferSlot) : -1;
          input.byteOffset = attr.byteOffset;
          input.perInstance = attr.vertexBufferSlot < bindings.size() && bindings[attr.vertexBufferSlot].instanceDivisor > 0;
          input.instanceRate = attr.vertexBufferSlot < bindings.size() && bindings[attr.vertexBufferSlot].instanceDivisor <= uint32_t(INT32_MAX)
              ? int(bindings[attr.vertexBufferSlot].instanceDivisor) : 0;
          input.format = attr.format;
          input.used = true;
          input.genericEnabled = !attr.enabled;
          input.floatCastWrong = attr.floatCast && (VarTypeCompType(sig.varType) == CompType::UInt || VarTypeCompType(sig.varType) == CompType::SInt);
          std::memset(&input.genericValue, 0, sizeof(input.genericValue));
          if(input.genericEnabled)
          {
            if(sig.compCount > 4) continue;
            std::memcpy(&input.genericValue, &attr.genericValue, size_t(sig.compCount) * sizeof(uint32_t));
            input.perInstance = false; input.instanceRate = 0;
            input.format.compByteWidth = 4; input.format.compCount = uint8_t(sig.compCount);
            input.format.compType = VarTypeCompType(sig.varType); input.format.type = ResourceFormatType::Regular;
          }
          inputs.push_back(input);
        }
      }
    }
    else inputs = state.GetVertexInputs();
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
      std::cout << ",\"format_type\":" << uint32_t(input.format.type)
                << ",\"component_type\":" << uint32_t(input.format.compType)
                << ",\"component_count\":" << uint32_t(input.format.compCount)
                << ",\"component_byte_width\":" << uint32_t(input.format.compByteWidth)
                << ",\"element_size\":" << input.format.ElementSize()
                << ",\"bgra_order\":" << (input.format.BGRAOrder() ? "true" : "false")
                << ",\"float_cast_wrong\":" << (input.floatCastWrong ? "true" : "false")
                << ",\"generic_bits\":";
      if(input.genericEnabled)
      {
        uint32_t bits[4] = {};
        std::memcpy(bits, &input.genericValue, sizeof(bits));
        std::cout << '[';
        for(uint32_t c = 0; c < (std::min)(uint32_t(input.format.compCount), 4U); ++c)
        { if(c) std::cout << ','; std::cout << bits[c]; }
        std::cout << ']';
      }
      else std::cout << "null";
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
  std::cout << "],\"scissors\":";
  const rdcarray<Scissor> *scissors = nullptr;
  if(state.IsCaptureD3D11()) scissors = &controller->GetD3D11PipelineState()->rasterizer.scissors;
  else if(state.IsCaptureD3D12()) scissors = &controller->GetD3D12PipelineState()->rasterizer.scissors;
  else if(state.IsCaptureGL()) scissors = &controller->GetGLPipelineState()->rasterizer.scissors;
  const bool vulkan_scissors = state.IsCaptureVK();
  if(!scissors && !vulkan_scissors) std::cout << "null";
  else
  {
    const size_t scissor_count = scissors ? scissors->size() : controller->GetVulkanPipelineState()->viewportScissor.viewportScissors.size();
    std::cout << '[';
    for(size_t i = 0; i < scissor_count; ++i)
    {
      if(i) std::cout << ',';
      const Scissor &scissor = scissors ? (*scissors)[i] : controller->GetVulkanPipelineState()->viewportScissor.viewportScissors[i].scissor;
      std::cout << "{\"enabled\":" << (scissor.enabled ? "true" : "false")
                << ",\"x\":" << scissor.x << ",\"y\":" << scissor.y
                << ",\"width\":" << scissor.width << ",\"height\":" << scissor.height << '}';
    }
    std::cout << ']';
  }
  const bool common = state.IsCaptureD3D11() || state.IsCaptureD3D12() || state.IsCaptureGL() || state.IsCaptureVK();
  std::cout << ",\"fixed_state\":";
  if(!common) std::cout << "null";
  else
  {
    const RasterState raster = state.GetRasterState();
    const DepthTestState depth = state.GetDepthTestState();
    const auto faces = state.GetStencilFaces();
    auto number64 = [](double value) {
      if(std::isfinite(value)) std::cout << std::setprecision(std::numeric_limits<double>::max_digits10) << value;
      else std::cout << "null";
    };
    auto face = [](const StencilFace &value) {
      std::cout << "{\"fail\":" << uint32_t(value.failOperation) << ",\"depth_fail\":" << uint32_t(value.depthFailOperation)
                << ",\"pass\":" << uint32_t(value.passOperation) << ",\"function\":" << uint32_t(value.function)
                << ",\"reference\":" << value.reference << ",\"compare_mask\":" << value.compareMask
                << ",\"write_mask\":" << value.writeMask << '}';
    };
    auto equation = [](const BlendEquation &value) {
      std::cout << "{\"source\":" << uint32_t(value.source) << ",\"destination\":" << uint32_t(value.destination)
                << ",\"operation\":" << uint32_t(value.operation) << '}';
    };
    std::cout << "{\"raster\":{\"front_ccw\":" << (raster.frontCCW ? "true" : "false")
              << ",\"fill\":" << uint32_t(raster.fillMode) << ",\"cull\":" << uint32_t(raster.cullMode) << "},\"depth\":{\"enabled\":"
              << (depth.depthEnable ? "true" : "false") << ",\"function\":" << uint32_t(depth.depthFunction)
              << ",\"writes\":" << (depth.depthWrites ? "true" : "false") << ",\"bounds_enabled\":" << (depth.depthBounds ? "true" : "false")
              << ",\"min_bounds\":"; number64(depth.minDepthBounds); std::cout << ",\"max_bounds\":"; number64(depth.maxDepthBounds);
    std::cout << "},\"stencil_enabled\":" << (state.IsStencilTestEnabled() ? "true" : "false") << ",\"stencil_front\":";
    face(faces.first); std::cout << ",\"stencil_back\":"; face(faces.second);
    std::cout << ",\"independent_blend\":" << (state.IsIndependentBlendingEnabled() ? "true" : "false") << ",\"blend_factor\":[";
    const auto factors = state.GetBlendFactor();
    for(size_t i = 0; i < 4; ++i) { if(i) std::cout << ','; number(factors[i]); }
    std::cout << "],\"blends\":[";
    // GetColorBlends assumes the D3D blend array has an element even when empty.
    const bool has_blends = (!state.IsCaptureD3D11() || !controller->GetD3D11PipelineState()->outputMerger.blendState.blends.empty()) &&
                            (!state.IsCaptureD3D12() || !controller->GetD3D12PipelineState()->outputMerger.blendState.blends.empty());
    const rdcarray<ColorBlend> blends = has_blends ? state.GetColorBlends() : rdcarray<ColorBlend>();
    for(size_t i = 0; i < blends.size(); ++i)
    {
      if(i) std::cout << ',';
      const ColorBlend &blend = blends[i];
      std::cout << "{\"enabled\":" << (blend.enabled ? "true" : "false") << ",\"logic_enabled\":" << (blend.logicOperationEnabled ? "true" : "false")
                << ",\"logic_operation\":" << uint32_t(blend.logicOperation) << ",\"write_mask\":" << uint32_t(blend.writeMask) << ",\"color\":";
      equation(blend.colorBlend); std::cout << ",\"alpha\":"; equation(blend.alphaBlend); std::cout << '}';
    }
    std::cout << "]}";
  }
  std::cout << '}';
}

int run_structure_worker(ICaptureFile *file)
{
  std::cout << "\nRENDERDOCX/1 {\"type\":\"structure_ready\"}" << std::endl;
  std::string command;
  while(std::getline(std::cin, command))
  {
    if(command == "quit") break;
    if(command.compare(0, 17, "structure_buffer ") != 0) { error("Invalid structured worker request"); continue; }
    std::istringstream args(command.substr(17));
    std::string event_text, chunk_text, offset_text, length_text, item;
    uint64_t event = 0, chunk = 0, offset = 0, length = 0;
    if(!(args >> event_text >> chunk_text >> offset_text >> length_text) || !decimal(event_text, event) ||
       !decimal(chunk_text, chunk) || !decimal(offset_text, offset) || !decimal(length_text, length) ||
       event > UINT32_MAX || length > 65536)
    { error("Invalid structured buffer request"); continue; }
    const SDFile &full = file->GetStructuredData();
    if(chunk >= full.chunks.size() || !full.chunks[size_t(chunk)]) { error("Structured data is unavailable for this chunk"); continue; }
    const SDObject *object = full.chunks[size_t(chunk)];
    std::vector<const SDObject *> nodes{object};
    std::vector<uint64_t> path;
    bool valid = true;
    while(args >> item)
    {
      uint64_t index = 0;
      if(!decimal(item, index) || index >= object->NumChildren()) { valid = false; break; }
      object = object->GetChild(size_t(index));
      if(!object) { valid = false; break; }
      path.push_back(index);
      nodes.push_back(object);
    }
    if(!valid || object->type.basetype != SDBasic::Buffer || offset > object->type.byteSize ||
       length > object->type.byteSize - offset || object->data.basic.u >= full.buffers.size() ||
       !full.buffers[size_t(object->data.basic.u)] ||
       full.buffers[size_t(object->data.basic.u)]->size() != object->type.byteSize)
    { error("Structured buffer data is unavailable or range is invalid"); continue; }
    const bytebuf &bytes = *full.buffers[size_t(object->data.basic.u)];
    static const char hex[] = "0123456789abcdef";
    std::cout << "\nRENDERDOCX/1 {\"type\":\"structured_buffer\",\"event_id\":" << event
              << ",\"offset\":\"" << offset << "\",\"path\":[";
    for(size_t i = 0; i < path.size(); ++i) { if(i) std::cout << ','; std::cout << '"' << path[i] << '"'; }
    const SDChunk *source_chunk = full.chunks[size_t(chunk)];
    std::cout << "],\"chunk_metadata\":{\"chunk_id\":" << source_chunk->metadata.chunkID
              << ",\"length\":\"" << source_chunk->metadata.length << "\",\"thread_id\":\""
              << source_chunk->metadata.threadID << "\"},\"nodes\":[";
    const std::map<ResourceId, size_t> no_resources;
    for(size_t i = 0; i < nodes.size(); ++i) { if(i) std::cout << ','; write_structured_node(nodes[i], no_resources); }
    std::cout << "],\"hex\":\"";
    for(size_t i = size_t(offset); i < size_t(offset + length); ++i)
      std::cout << hex[bytes[i] >> 4] << hex[bytes[i] & 15];
    std::cout << "\"}" << std::endl;
  }
  return 0;
}

int run(const std::string &capture, bool structure_worker, const std::string &remote_url, const ReplayOptions &options)
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
  if(structure_worker) return run_structure_worker(replay.file);
  if(!remote_url.empty())
  {
    result = RENDERDOC_CreateRemoteServerConnection(remote_url.c_str(), &replay.remote);
    if(!result.OK() || !replay.remote) { error(result.OK() ? "Remote server returned no connection" : result_message(result)); return 1; }
    replay.remote_path = replay.remote->CopyCaptureToRemote(capture.c_str(), nullptr);
    if(replay.remote_path.empty()) { error("Could not copy capture to remote replay server"); return 1; }
    auto opened = replay.remote->OpenCapture(IRemoteServer::NoPreference, replay.remote_path, options, nullptr);
    if(!opened.first.OK()) { error(result_message(opened.first)); return 1; }
    replay.controller = opened.second;
  }
  else
  {
    auto opened = replay.file->OpenCapture(options, nullptr);
    if(!opened.first.OK()) { error(result_message(opened.first)); return 1; }
    replay.controller = opened.second;
  }
  if(!replay.controller) { error("RenderDoc returned no replay controller"); return 1; }
  DebugTraceScope debug{replay.controller};
  ShaderReplacementScope replacements{replay.controller};
  CustomShaderScope custom_shader{replay.controller};
  if(!structure_worker) { replay.file->Shutdown(); replay.file = nullptr; }
  std::set<uint32_t> event_ids;
  std::vector<TextureRef> texture_ids;
  std::vector<BufferRef> buffer_ids;
  std::map<uint32_t, uint32_t> event_chunks;
  std::map<uint32_t, const SDObject *> event_annotations;
  std::map<uint32_t, uint32_t> draw_instances;
  std::map<uint32_t, uint32_t> draw_vertices;
  std::set<uint32_t> indexed_draws;
  std::map<uint32_t, const ActionDescription *> dispatch_actions;
  write_snapshot(replay.controller, options, event_ids, texture_ids, buffer_ids, event_chunks, event_annotations, draw_instances, draw_vertices, indexed_draws, dispatch_actions);
  std::map<ResourceId, size_t> resource_indices;
  const auto &resources = replay.controller->GetResources();
  for(size_t i = 0; i < resources.size(); ++i) resource_indices.emplace(resources[i].resourceId, i);
  bool selected = false;
  bool symbols_initialised = false;
  uint32_t selected_event = 0;
  bool debug_finished = false;
  bool mesh_ready = false;
  MeshFormat selected_mesh;
  uint32_t mesh_instance = 0, mesh_view = 0, mesh_stage = 0;
  bool history_ready = false;
  uint32_t history_event = 0, history_index = 0, history_mip = 0, history_slice = 0, history_sample = 0, history_x = 0, history_y = 0, history_cast = 0;
  rdcarray<PixelModification> cached_history;
  rdcarray<CounterResult> counter_results;
  uint64_t counter_generation = 0;
  bool counters_ready = false;
  bool raw_ready = false;
  uint32_t raw_event = 0, raw_index = 0, raw_mip = 0, raw_slice = 0, raw_sample = 0;
  bytebuf raw_cache;

  std::string command;
  while(std::getline(std::cin, command))
  {
    if(command == "quit") break;
    // A live debugger can hold backend replay state. Do not allow unrelated replay
    // operations to mutate that state between ContinueDebug calls.
    if(debug.trace && command != "debug_next" && command != "debug_stop" && command != "debug_messages" &&
       command.compare(0, 13, "debug_vertex ") != 0 && command.compare(0, 12, "debug_pixel ") != 0 &&
       command.compare(0, 14, "debug_compute ") != 0 && command.compare(0, 11, "debug_mesh ") != 0 &&
       command.compare(0, 7, "select ") != 0)
    { error("Stop the shader debug trace before other replay operations"); continue; }
    if(command == "debug_messages")
    {
      const auto messages = replay.controller->GetDebugMessages();
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"debug_messages\",\"messages\":[";
      for(size_t i = 0; i < messages.size(); ++i) { if(i) std::cout << ','; write_debug_message(messages[i]); }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command == "dependencies" || command.compare(0, 18, "dependencies_save ") == 0)
    {
      const bool saving = command != "dependencies";
      std::string operation, encoded, extra, output;
      bytebuf bytes;
      if(saving)
      {
        std::istringstream args(command.substr(18));
        if(!(args >> operation >> encoded) || (args >> extra) || (operation != "embed" && operation != "remove") ||
           !decode_section_hex(encoded, bytes) || bytes.empty() || std::find(bytes.begin(), bytes.end(), byte(0)) != bytes.end())
        { error("Invalid dependency export request"); continue; }
        output.assign(bytes.begin(), bytes.end());
      }
      ICaptureFile *file = nullptr;
      ICaptureAccess *access = replay.remote;
      if(!access)
      {
        file = RENDERDOC_OpenCaptureFile();
        if(!file) { error("Could not open capture dependencies"); continue; }
        ResultDetails opened = file->OpenFile(saving ? output.c_str() : capture.c_str(), "rdc", nullptr);
        if(!opened.OK()) { std::string message = result_message(opened); file->Shutdown(); error(message); continue; }
        access = file;
      }
      const bool embedded = access->HasEmbeddedDependencies();
      const bool pending = access->HasPendingDependencies();
      if(saving && ((operation == "embed" && !pending) || (operation == "remove" && !embedded)))
      { if(file) file->Shutdown(); error(operation == "embed" ? "No pending dependencies to embed" : "No embedded dependencies to remove"); continue; }
      if(saving)
      {
        ResultDetails changed = operation == "embed" ? access->EmbedDependenciesIntoCapture() : access->RemoveDependenciesFromCapture();
        if(!changed.OK()) { std::string message = result_message(changed); if(file) file->Shutdown(); error(message); continue; }
        if(replay.remote) replay.remote->CopyCaptureFromRemote(replay.remote_path, output.c_str(), nullptr);
        if(file) { file->Shutdown(); file = nullptr; }
        ICaptureFile *check = RENDERDOC_OpenCaptureFile();
        if(!check) { error("Could not verify exported capture"); continue; }
        ResultDetails verified = check->OpenFile(output.c_str(), "rdc", nullptr);
        const std::string verification_error = verified.OK() ? std::string() : result_message(verified);
        const bool exported_embedded = verified.OK() && check->HasEmbeddedDependencies();
        check->Shutdown();
        if(!verification_error.empty()) { error("Could not verify exported capture: " + verification_error); continue; }
        if(exported_embedded != (operation == "embed")) { error("Exported capture has an unexpected dependency state"); continue; }
        std::cout << "\nRENDERDOCX/1 {\"type\":\"dependencies_saved\"}" << std::endl;
      }
      else
      {
        const auto names = access->GetPendingDependenciesNicknames();
        std::cout << "\nRENDERDOCX/1 {\"type\":\"dependencies\",\"embedded\":" << (embedded ? "true" : "false")
                  << ",\"pending\":" << (pending ? "true" : "false") << ",\"nicknames\":[";
        for(size_t i = 0; i < names.size(); ++i) { if(i) std::cout << ','; json_string(names[i]); }
        std::cout << "]}" << std::endl;
        if(file) file->Shutdown();
      }
      continue;
    }
    if(command == "remote_gpus")
    {
      if(!replay.remote) { error("This capture does not use a remote server"); continue; }
      const auto gpus = replay.remote->GetAvailableGPUs();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"gpus\",\"gpus\":[";
      for(size_t i = 0; i < gpus.size(); ++i)
      {
        if(i) std::cout << ',';
        std::cout << "{\"vendor\":" << uint32_t(gpus[i].vendor) << ",\"device_id\":" << gpus[i].deviceID << ",\"driver\":"; json_string(gpus[i].driver);
        std::cout << ",\"name\":"; json_string(gpus[i].name); std::cout << ",\"apis\":[";
        for(size_t j = 0; j < gpus[i].apis.size(); ++j) { if(j) std::cout << ','; std::cout << uint32_t(gpus[i].apis[j]); }
        std::cout << "]}";
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 12, "remote_list ") == 0 || command.compare(0, 12, "remote_copy ") == 0)
    {
      if(!replay.remote) { error("This capture does not use a remote server"); continue; }
      const bool copy = command.compare(0, 12, "remote_copy ") == 0;
      std::istringstream args(command.substr(12));
      std::string first, second, extra;
      bytebuf first_bytes, second_bytes;
      if(!(args >> first) || (copy && !(args >> second)) || (args >> extra) ||
         !decode_section_hex(first, first_bytes) || (copy && !decode_section_hex(second, second_bytes)))
      { error("Invalid remote file request"); continue; }
      if(std::find(first_bytes.begin(), first_bytes.end(), byte(0)) != first_bytes.end() ||
         std::find(second_bytes.begin(), second_bytes.end(), byte(0)) != second_bytes.end())
      { error("Invalid remote path"); continue; }
      const std::string first_path(first_bytes.begin(), first_bytes.end());
      const std::string second_path(second_bytes.begin(), second_bytes.end());
      if(copy)
      {
        if(first_path.empty() || second_path.empty()) { error("Invalid remote copy path"); continue; }
        replay.remote->CopyCaptureFromRemote(first_path.c_str(), second_path.c_str(), nullptr);
        std::cout << "\nRENDERDOCX/1 {\"type\":\"remote_copy\"}" << std::endl;
        continue;
      }
      const rdcstr home = replay.remote->GetHomeFolder();
      const rdcstr path = first_path.empty() ? home : rdcstr(first_path.c_str());
      if(path.empty()) { error("Remote server returned an empty directory"); continue; }
      const auto files = replay.remote->ListFolder(path);
      const auto drivers = replay.remote->RemoteSupportedReplays();
      const auto proxies = replay.remote->LocalProxies();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"remote_directory\",\"home\":"; json_string(home);
      std::cout << ",\"path\":"; json_string(path);
      std::cout << ",\"drivers\":[";
      for(size_t i = 0; i < drivers.size(); ++i) { if(i) std::cout << ','; json_string(drivers[i]); }
      std::cout << "],\"proxies\":[";
      for(size_t i = 0; i < proxies.size(); ++i) { if(i) std::cout << ','; json_string(proxies[i]); }
      std::cout << "],\"entries\":[";
      for(size_t i = 0; i < files.size(); ++i)
      {
        if(i) std::cout << ',';
        std::cout << "{\"name\":"; json_string(files[i].filename);
        std::cout << ",\"flags\":" << uint32_t(files[i].flags) << ",\"last_modified\":" << files[i].lastmod
                  << ",\"size\":\"" << files[i].size << "\"}";
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 10, "callstack ") == 0)
    {
      std::istringstream args(command.substr(10));
      std::string event_text, resolve_text, extra;
      uint64_t event = 0, resolve = 0;
      if(!(args >> event_text >> resolve_text) || !decimal(event_text, event) || event > UINT32_MAX ||
         !decimal(resolve_text, resolve) || resolve > 1 || (args >> extra))
      { error("Invalid callstack request"); continue; }
      auto found = event_chunks.find(uint32_t(event));
      const SDFile &structured = replay.controller->GetStructuredFile();
      if(found == event_chunks.end() || found->second >= structured.chunks.size() || !structured.chunks[found->second])
      { error("Callstack is unavailable for this event"); continue; }
      ICaptureAccess *access = replay.remote;
      if(!access)
      {
        if(!replay.file)
        {
          replay.file = RENDERDOC_OpenCaptureFile();
          if(!replay.file) { error("Could not open capture for symbol resolution"); continue; }
          ResultDetails opened = replay.file->OpenFile(capture.c_str(), "rdc", nullptr);
          if(!opened.OK()) { std::string message = result_message(opened); replay.file->Shutdown(); replay.file = nullptr; error(message); continue; }
        }
        access = replay.file;
      }
      const auto &stack = structured.chunks[found->second]->metadata.callstack;
      const bool available = access->HasCallstacks();
      if(resolve && available && !stack.empty() && !symbols_initialised)
      {
        ResultDetails init = access->InitResolver(false, nullptr);
        if(!init.OK()) { error(result_message(init)); continue; }
        symbols_initialised = true;
      }
      rdcarray<rdcstr> symbols;
      if(resolve && symbols_initialised && !stack.empty()) symbols = access->GetResolve(stack);
      const bool resolved = resolve && symbols_initialised && !stack.empty() && symbols.size() == stack.size();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"callstack\",\"event_id\":" << event
                << ",\"available\":" << (available ? "true" : "false")
                << ",\"resolved\":" << (resolved ? "true" : "false") << ",\"frames\":[";
      for(size_t i = 0; i < stack.size(); ++i)
      {
        if(i) std::cout << ',';
        std::cout << "{\"address\":\"" << stack[i] << "\",\"symbol\":";
        if(resolved) json_string(symbols[i]); else std::cout << "null";
        std::cout << '}';
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 13, "section_read ") == 0 || command.compare(0, 14, "section_write ") == 0)
    {
      const bool writing = command.compare(0, 14, "section_write ") == 0;
      if(writing && replay.file) { replay.file->Shutdown(); replay.file = nullptr; symbols_initialised = false; }
      std::istringstream arguments(command.substr(writing ? 14 : 13));
      std::string name, expected, payload, extra;
      if(!(arguments >> name) || (name != "notes" && name != "bookmarks" && name != "renames" && name != "edits") ||
         (writing && !(arguments >> expected >> payload)) || (arguments >> extra))
      { error("Invalid annotation section request"); continue; }
      const SectionType type = name == "notes" ? SectionType::Notes : (name == "bookmarks" ? SectionType::Bookmarks : (name == "renames" ? SectionType::ResourceRenames : SectionType::EditedShaders));
      bytebuf expected_bytes, new_bytes;
      if(writing && ((expected != "-" && !decode_section_hex(expected, expected_bytes)) || !decode_section_hex(payload, new_bytes)))
      { error("Invalid annotation section encoding"); continue; }
      ICaptureFile *file = RENDERDOC_OpenCaptureFile();
      if(!file) { error("RenderDoc could not open capture metadata"); continue; }
      ResultDetails open = file->OpenFile(capture.c_str(), "rdc", nullptr);
      if(!open.OK()) { std::string message = result_message(open); file->Shutdown(); error(message); continue; }
      const int32_t section = file->FindSectionByType(type);
      const bool exists = section >= 0;
      const uint64_t version = exists ? file->GetSectionProperties(section).version : 0;
      bytebuf original;
      if(exists) original = file->GetSectionContents(section);
      if(writing && ((expected == "-") != !exists || (exists && (version != 1 || original != expected_bytes))))
      { file->Shutdown(); error("Annotation section changed or has an unsupported version"); continue; }
      if(writing)
      {
        SectionProperties props; props.type = type; props.version = 1;
        ResultDetails saved = file->WriteSection(props, new_bytes);
        if(!saved.OK()) { std::string message = result_message(saved); file->Shutdown(); error(message); continue; }
        original = new_bytes;
      }
      file->Shutdown();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"annotation_section\",\"name\":";
      json_string(name.c_str(), name.size());
      std::cout << ",\"exists\":" << ((exists || writing) ? "true" : "false") << ",\"version\":\"" << (writing ? 1 : version) << "\",\"hex\":\"";
      write_section_hex(original);
      std::cout << "\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 12, "descriptors ") == 0)
    {
      std::istringstream arguments(command.substr(12));
      std::string mode, index_text, offset_text, limit_text, extra;
      uint64_t index = 0, offset = 0, limit = 0;
      if(!(arguments >> mode >> index_text >> offset_text >> limit_text) || (arguments >> extra) ||
         (mode != "access" && mode != "store") || !decimal(index_text, index) || !decimal(offset_text, offset) ||
         !decimal(limit_text, limit) || limit == 0 || limit > 128 || !selected)
      { error("Invalid descriptor request or no selected event"); continue; }
      const auto &accesses = replay.controller->GetDescriptorAccess();
      const auto &stores = replay.controller->GetDescriptorStores();
      if((mode == "access" && index != 0) || (mode == "store" && index >= stores.size()))
      { error("Invalid descriptor store"); continue; }
      const uint64_t total = mode == "access" ? accesses.size() : stores[size_t(index)].descriptorCount;
      if(offset > total || (mode == "store" && stores[size_t(index)].descriptorByteSize == 0))
      { error("Descriptor range is invalid or store has variable-sized descriptors"); continue; }
      const size_t count = size_t((std::min)(limit, total - offset));
      if(mode == "store" && count > 0 && uint64_t(stores[size_t(index)].firstDescriptorOffset) +
           (offset + count - 1) * uint64_t(stores[size_t(index)].descriptorByteSize) > UINT32_MAX)
      { error("Descriptor range exceeds upstream offset limits"); continue; }
      std::map<ResourceId, size_t> access_stores;
      for(const auto &access : accesses)
        if(access.descriptorStore != ResourceId() && access_stores.find(access.descriptorStore) == access_stores.end())
          access_stores.emplace(access.descriptorStore, access_stores.size());
      struct DescriptorRow { DescriptorAccess access; bool has_access = false; rdcarray<Descriptor> descriptors; rdcarray<SamplerDescriptor> samplers; rdcarray<DescriptorLogicalLocation> locations; uint32_t byte_offset = 0; };
      std::vector<DescriptorRow> rows(count);
      for(size_t i = 0; i < count; ++i)
      {
        DescriptorRow &row = rows[i];
        ResourceId store;
        DescriptorRange range;
        if(mode == "access")
        {
          row.has_access = true; row.access = accesses[size_t(offset) + i];
          store = row.access.descriptorStore; range = DescriptorRange(row.access);
        }
        else
        {
          const auto &desc = stores[size_t(index)]; store = desc.resourceId;
          range.offset = uint32_t(uint64_t(desc.firstDescriptorOffset) + (offset + i) * desc.descriptorByteSize);
          range.descriptorSize = desc.descriptorByteSize; range.type = DescriptorType::Unknown;
        }
        row.byte_offset = range.offset;
        if(store == ResourceId() || range.descriptorSize == 0) continue;
        rdcarray<DescriptorRange> ranges = {range};
        row.descriptors = replay.controller->GetDescriptors(store, ranges);
        row.locations = replay.controller->GetDescriptorLocations(store, ranges);
        DescriptorType sampler_type = row.has_access ? row.access.type : (row.descriptors.empty() ? DescriptorType::Unknown : row.descriptors[0].type);
        if(sampler_type == DescriptorType::Sampler || sampler_type == DescriptorType::ImageSampler)
        { range.type = sampler_type; ranges[0] = range; row.samplers = replay.controller->GetSamplerDescriptors(store, ranges); }
      }
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"descriptors\",\"event_id\":" << selected_event << ",\"kind\":";
      json_string(mode.c_str(), mode.size());
      std::cout << ",\"store_index\":"; if(mode == "store") std::cout << index; else std::cout << "null";
      std::cout << ",\"offset\":\"" << offset << "\",\"total\":\"" << total << "\",\"entries\":[";
      for(size_t i = 0; i < rows.size(); ++i)
      {
        if(i) std::cout << ',';
        const auto &row = rows[i];
        std::cout << "{\"byte_offset\":" << row.byte_offset << ",\"access\":";
        if(!row.has_access) std::cout << "null";
        else
        {
          const auto &a = row.access;
          const auto store = access_stores.find(a.descriptorStore), resource = resource_indices.find(a.descriptorStore);
          std::cout << "{\"stage\":" << uint32_t(a.stage) << ",\"type\":" << uint32_t(a.type) << ",\"binding\":" << a.index
                    << ",\"array_element\":" << a.arrayElement << ",\"byte_size\":" << a.byteSize
                    << ",\"statically_unused\":" << (a.staticallyUnused ? "true" : "false") << ",\"store_ordinal\":";
          if(store == access_stores.end()) std::cout << "null"; else std::cout << store->second;
          std::cout << ",\"store_resource\":";
          if(resource == resource_indices.end()) std::cout << "null"; else std::cout << resource->second;
          std::cout << '}';
        }
        std::cout << ",\"descriptor\":";
        if(row.descriptors.empty()) std::cout << "null"; else write_descriptor(row.descriptors[0], resource_indices);
        std::cout << ",\"sampler\":";
        if(row.samplers.empty()) std::cout << "null"; else write_sampler(row.samplers[0], resource_indices);
        std::cout << ",\"location\":";
        if(row.locations.empty()) std::cout << "null";
        else { const auto &loc = row.locations[0]; std::cout << "{\"stage_mask\":" << uint32_t(loc.stageMask) << ",\"category\":" << uint32_t(loc.category) << ",\"fixed_bind\":" << loc.fixedBindNumber << ",\"name\":"; json_string(loc.logicalBindName.c_str(), loc.logicalBindName.size()); std::cout << '}'; }
        std::cout << '}';
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 13, "debug_vertex ") == 0)
    {
      std::istringstream arguments(command.substr(13));
      std::string vertex_text, instance_text, index_text, view_text, extra;
      uint64_t vertex = 0, instance = 0, index = 0, view = 0;
      if(!(arguments >> vertex_text >> instance_text >> index_text >> view_text) || (arguments >> extra) ||
         !decimal(vertex_text, vertex) || !decimal(instance_text, instance) ||
         !decimal(index_text, index) || !decimal(view_text, view) ||
         vertex > UINT32_MAX || instance > UINT32_MAX || index > UINT32_MAX || view > UINT32_MAX)
      { error("Invalid vertex debug request"); continue; }
      auto vertices = draw_vertices.find(selected_event);
      auto instances = draw_instances.find(selected_event);
      if(!selected || vertices == draw_vertices.end() || instances == draw_instances.end())
      { error("Select a draw event before debugging a vertex"); continue; }
      if(vertex >= vertices->second || instance >= instances->second)
      { error("Vertex or instance is out of range"); continue; }
      if(view >= replay.controller->GetPipelineState().MultiviewBroadcastCount())
      { error("Vertex debug view is out of range"); continue; }
      if(replay.controller->GetPipelineState().GetShader(ShaderStage::Vertex) == ResourceId())
      { error("No vertex shader is bound"); continue; }
      debug.reset();
      debug_finished = false;
      mesh_ready = false;
      debug.trace = replay.controller->DebugVertex(uint32_t(vertex), uint32_t(instance), uint32_t(index), uint32_t(view));
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(!debug.trace || !debug.trace->debugger)
      { debug.reset(); error("RenderDoc could not create a vertex debug trace"); continue; }
      if(debug.trace->stage != ShaderStage::Vertex)
      { debug.reset(); error("RenderDoc returned a different shader stage"); continue; }
      write_debug_trace(selected_event, *debug.trace);
      continue;
    }
    if(command.compare(0, 12, "debug_pixel ") == 0)
    {
      std::istringstream arguments(command.substr(12));
      std::string x_text, y_text, sample_text, primitive_text, view_text, extra;
      uint64_t x = 0, y = 0, sample = 0, primitive = 0, view = 0;
      if(!(arguments >> x_text >> y_text >> sample_text >> primitive_text >> view_text) || (arguments >> extra) ||
         !decimal(x_text, x) || !decimal(y_text, y) || !decimal(sample_text, sample) ||
         !decimal(primitive_text, primitive) || !decimal(view_text, view) ||
         x > UINT32_MAX || y > UINT32_MAX || sample > UINT32_MAX || primitive > UINT32_MAX || view > UINT32_MAX)
      { error("Invalid pixel debug request"); continue; }
      if(!selected || draw_vertices.find(selected_event) == draw_vertices.end())
      { error("Select a draw event before debugging a pixel"); continue; }
      if(replay.controller->GetPipelineState().GetShader(ShaderStage::Pixel) == ResourceId())
      { error("No pixel shader is bound"); continue; }
      DebugPixelInputs inputs;
      inputs.sample = uint32_t(sample); inputs.primitive = uint32_t(primitive); inputs.view = uint32_t(view);
      debug.reset();
      debug_finished = false;
      mesh_ready = false;
      debug.trace = replay.controller->DebugPixel(uint32_t(x), uint32_t(y), inputs);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(!debug.trace || !debug.trace->debugger)
      { debug.reset(); error("RenderDoc could not create a pixel debug trace"); continue; }
      if(debug.trace->stage != ShaderStage::Pixel)
      { debug.reset(); error("RenderDoc returned a different shader stage"); continue; }
      write_debug_trace(selected_event, *debug.trace);
      continue;
    }
    if(command.compare(0, 14, "debug_compute ") == 0)
    {
      std::istringstream arguments(command.substr(14));
      std::string text[6], extra;
      rdcfixedarray<uint32_t, 3> group, thread;
      bool valid = true;
      for(size_t i = 0; i < 6; ++i)
      {
        uint64_t value = 0;
        if(!(arguments >> text[i]) || !decimal(text[i], value) || value > UINT32_MAX) { valid = false; break; }
        if(i < 3) group[i] = uint32_t(value); else thread[i - 3] = uint32_t(value);
      }
      if(!valid || (arguments >> extra)) { error("Invalid compute debug request"); continue; }
      auto action = dispatch_actions.find(selected_event);
      if(!selected || action == dispatch_actions.end() || !(action->second->flags & ActionFlags::Dispatch)) { error("Select a compute dispatch event first"); continue; }
      const PipeState &state = replay.controller->GetPipelineState();
      if(state.GetShader(ShaderStage::Compute) == ResourceId()) { error("No compute shader is bound"); continue; }
      const ShaderReflection *reflection = state.GetShaderReflection(ShaderStage::Compute);
      if(!reflection) { error("Compute shader reflection is unavailable"); continue; }
      const auto &groups = action->second->dispatchDimension;
      const auto &threads = action->second->dispatchThreadsDimension[0] == 0
                                ? reflection->dispatchThreadsDimension : action->second->dispatchThreadsDimension;
      for(size_t i = 0; i < 3; ++i)
        if(groups[i] == 0 || threads[i] == 0 || group[i] >= groups[i] || thread[i] >= threads[i]) valid = false;
      if(!valid) { error("Compute group or thread is out of range or its dimensions are unavailable"); continue; }
      debug.reset(); debug_finished = false; mesh_ready = false;
      debug.trace = replay.controller->DebugThread(group, thread);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(!debug.trace || !debug.trace->debugger)
      { debug.reset(); error("RenderDoc could not create a compute debug trace"); continue; }
      if(debug.trace->stage != ShaderStage::Compute)
      { debug.reset(); error("RenderDoc returned a different shader stage"); continue; }
      write_debug_trace(selected_event, *debug.trace);
      continue;
    }
    if(command.compare(0, 11, "debug_mesh ") == 0)
    {
      std::istringstream arguments(command.substr(11));
      std::string text[6], extra;
      rdcfixedarray<uint32_t, 3> group, thread;
      bool valid = true;
      for(size_t i = 0; i < 6; ++i)
      {
        uint64_t value = 0;
        if(!(arguments >> text[i]) || !decimal(text[i], value) || value > UINT32_MAX) { valid = false; break; }
        if(i < 3) group[i] = uint32_t(value); else thread[i - 3] = uint32_t(value);
      }
      if(!valid || (arguments >> extra)) { error("Invalid mesh debug request"); continue; }
      auto action = dispatch_actions.find(selected_event);
      if(!selected || action == dispatch_actions.end() || !(action->second->flags & ActionFlags::MeshDispatch))
      { error("Select a mesh dispatch event first"); continue; }
      const PipeState &state = replay.controller->GetPipelineState();
      if(state.GetShader(ShaderStage::Mesh) == ResourceId()) { error("No mesh shader is bound"); continue; }
      const ShaderReflection *reflection = state.GetShaderReflection(ShaderStage::Mesh);
      if(!reflection) { error("Mesh shader reflection is unavailable"); continue; }
      const auto &groups = action->second->dispatchDimension;
      const auto &threads = action->second->dispatchThreadsDimension[0] == 0
                                ? reflection->dispatchThreadsDimension : action->second->dispatchThreadsDimension;
      for(size_t i = 0; i < 3; ++i)
        if(groups[i] == 0 || threads[i] == 0 || group[i] >= groups[i] || thread[i] >= threads[i]) valid = false;
      if(!valid) { error("Mesh group or thread is out of range or its dimensions are unavailable"); continue; }
      debug.reset(); debug_finished = false; mesh_ready = false;
      debug.trace = replay.controller->DebugMeshThread(group, thread);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(!debug.trace || !debug.trace->debugger)
      { debug.reset(); error("RenderDoc could not create a mesh debug trace"); continue; }
      if(debug.trace->stage != ShaderStage::Mesh)
      { debug.reset(); error("RenderDoc returned a different shader stage"); continue; }
      write_debug_trace(selected_event, *debug.trace);
      continue;
    }
    if(command == "debug_next")
    {
      if(!debug.trace || !debug.trace->debugger) { error("No shader debug trace is active"); continue; }
      rdcarray<ShaderDebugState> states;
      if(!debug_finished) states = replay.controller->ContinueDebug(debug.trace->debugger);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(states.empty()) debug_finished = true;
      std::cout << "\nRENDERDOCX/1 {\"type\":\"debug_states\",\"event_id\":" << selected_event
                << ",\"finished\":" << (debug_finished ? "true" : "false") << ",\"states\":[";
      for(size_t i = 0; i < states.size(); ++i)
      {
        if(i) std::cout << ',';
        const auto &state = states[i];
        std::cout << "{\"step\":" << state.stepIndex << ",\"next_instruction\":" << state.nextInstruction
                  << ",\"flags\":" << uint32_t(state.flags) << ",\"callstack\":[";
        for(size_t j = 0; j < state.callstack.size(); ++j)
        { if(j) std::cout << ','; json_string(state.callstack[j]); }
        std::cout << "],\"changes\":[";
        for(size_t j = 0; j < state.changes.size(); ++j)
        {
          if(j) std::cout << ',';
          std::cout << "{\"before\":"; write_debug_variable(state.changes[j].before);
          std::cout << ",\"after\":"; write_debug_variable(state.changes[j].after);
          std::cout << '}';
        }
        std::cout << "]}";
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command == "debug_stop")
    {
      debug.reset();
      debug_finished = false;
      std::cout << "\nRENDERDOCX/1 {\"type\":\"debug_stopped\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 14, "counter_fetch ") == 0)
    {
      std::istringstream arguments(command.substr(14));
      std::string value;
      const auto available = replay.controller->EnumerateCounters();
      std::set<uint32_t> valid;
      for(GPUCounter counter : available) valid.insert(uint32_t(counter));
      std::set<uint32_t> unique;
      rdcarray<GPUCounter> requested;
      bool valid_request = true;
      while(arguments >> value)
      {
        uint64_t id = 0;
        if(!decimal(value, id) || id > UINT32_MAX || !valid.count(uint32_t(id)) || !unique.insert(uint32_t(id)).second)
        { valid_request = false; break; }
        requested.push_back(GPUCounter(uint32_t(id)));
      }
      if(!valid_request || requested.empty()) { error("Invalid GPU counter selection"); continue; }
      counters_ready = false;
      counter_results = replay.controller->FetchCounters(requested);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      mesh_ready = false;
      history_ready = false;
      raw_ready = false;
      raw_cache = bytebuf();
      cached_history = rdcarray<PixelModification>();
      if(selected) replay.controller->SetFrameEvent(selected_event, true);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      ++counter_generation;
      counters_ready = true;
      std::cout << "\nRENDERDOCX/1 {\"type\":\"counter_fetch\",\"generation\":\"" << counter_generation
                << "\",\"total\":\"" << counter_results.size() << "\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 13, "counter_page ") == 0)
    {
      std::istringstream arguments(command.substr(13));
      std::string gen_text, offset_text, limit_text, extra;
      uint64_t gen = 0, offset = 0, limit = 0;
      if(!(arguments >> gen_text >> offset_text >> limit_text) || (arguments >> extra) ||
         !decimal(gen_text, gen) || !decimal(offset_text, offset) || !decimal(limit_text, limit) ||
         !counters_ready || gen != counter_generation || offset > counter_results.size() || limit == 0 || limit > 128)
      { error("Invalid or stale GPU counter page"); continue; }
      const size_t start = size_t(offset), end = start + (std::min)(size_t(limit), counter_results.size() - start);
      bool supported = true;
      for(size_t i = start; i < end; ++i)
      {
        const auto width = replay.controller->DescribeCounter(counter_results[i].counter).resultByteWidth;
        if(width != 4 && width != 8) { supported = false; break; }
      }
      if(!supported) { error("Unsupported counter result width"); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"counter_page\",\"generation\":\"" << gen << "\",\"offset\":\"" << offset
                << "\",\"total\":\"" << counter_results.size() << "\",\"entries\":[";
      for(size_t i = start; i < end; ++i)
      {
        if(i != start) std::cout << ',';
        const CounterResult &entry = counter_results[i];
        const CounterDescription desc = replay.controller->DescribeCounter(entry.counter);
        uint64_t bits = 0;
        if(desc.resultByteWidth == 4) { uint32_t raw; std::memcpy(&raw, &entry.value, sizeof(raw)); bits = raw; }
        else if(desc.resultByteWidth == 8) std::memcpy(&bits, &entry.value, sizeof(bits));
        std::cout << "{\"event_id\":" << entry.eventId << ",\"counter\":" << uint32_t(entry.counter) << ",\"bits\":\"" << bits << "\"}";
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 15, "timeline_usage ") == 0)
    {
      const std::string text = command.substr(15);
      uint64_t index = 0;
      if(!decimal(text, index) || index >= resources.size()) { error("Invalid resource index"); continue; }
      const auto usage = replay.controller->GetUsage(resources[size_t(index)].resourceId);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      const bool tracked = !(usage.size() == 1 && usage[0].eventId == 0 && usage[0].usage == ResourceUsage::Unused);
      std::cout << "\nRENDERDOCX/1 {\"type\":\"timeline_usage\",\"index\":" << index
                << ",\"tracked\":" << (tracked ? "true" : "false") << ",\"entries\":[";
      if(tracked) for(size_t i = 0; i < usage.size(); ++i)
      {
        if(i) std::cout << ',';
        std::cout << "{\"event_id\":" << usage[i].eventId << ",\"usage\":" << uint32_t(usage[i].usage) << '}';
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command == "timeline_history")
    {
      if(!history_ready || !selected || history_event != selected_event) { error("No pixel history is currently cached"); continue; }
      std::set<uint32_t> events, passed_events, failed_events;
      for(const PixelModification &mod : cached_history)
      {
        events.insert(mod.eventId);
        (mod.Passed() ? passed_events : failed_events).insert(mod.eventId);
      }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"timeline_history\",\"event_id\":" << history_event
                << ",\"index\":" << history_index << ",\"mip\":" << history_mip << ",\"slice\":" << history_slice
                << ",\"sample\":" << history_sample << ",\"x\":" << history_x << ",\"y\":" << history_y
                << ",\"modifications\":\"" << cached_history.size() << "\",\"events\":[";
      bool first = true;
      for(uint32_t event : events) { if(!first) std::cout << ','; first = false; std::cout << event; }
      std::cout << "],\"passed_events\":[";
      first = true;
      for(uint32_t event : passed_events) { if(!first) std::cout << ','; first = false; std::cout << event; }
      std::cout << "],\"failed_events\":[";
      first = true;
      for(uint32_t event : failed_events) { if(!first) std::cout << ','; first = false; std::cout << event; }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 6, "usage ") == 0)
    {
      std::istringstream arguments(command.substr(6));
      std::string index_text, offset_text, limit_text, extra;
      uint64_t index = 0, offset = 0, limit = 0;
      if(!(arguments >> index_text >> offset_text >> limit_text) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(offset_text, offset) || !decimal(limit_text, limit) ||
         index >= resources.size() || limit == 0 || limit > 128)
      { error("Invalid resource usage request"); continue; }
      const rdcarray<EventUsage> usage = replay.controller->GetUsage(resources[size_t(index)].resourceId);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(offset > usage.size()) { error("Resource usage offset is out of range"); continue; }
      const bool tracked = !(usage.size() == 1 && usage[0].eventId == 0 && usage[0].usage == ResourceUsage::Unused);
      const size_t start = size_t(offset), end = start + (std::min)(size_t(limit), usage.size() - start);
      std::cout << "\nRENDERDOCX/1 {\"type\":\"resource_usage\",\"index\":" << index
                << ",\"offset\":\"" << offset << "\",\"total\":\"" << usage.size() << "\",\"tracked\":" << (tracked ? "true" : "false") << ",\"entries\":[";
      for(size_t i = start; i < end; ++i)
      {
        if(i != start) std::cout << ',';
        std::cout << "{\"event_id\":" << usage[i].eventId << ",\"usage\":" << uint32_t(usage[i].usage) << '}';
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 5, "mesh ") == 0)
    {
      std::istringstream arguments(command.substr(5));
      std::string instance_text, view_text, stage_text, extra;
      uint64_t instance = 0, view = 0, stage = 0;
      if(!(arguments >> instance_text >> view_text >> stage_text) || (arguments >> extra) ||
         !decimal(instance_text, instance) || !decimal(view_text, view) || !decimal(stage_text, stage) ||
         instance > UINT32_MAX || view > UINT32_MAX || stage < 1 || stage > 4)
      { error("Invalid mesh request"); continue; }
      auto draw = draw_instances.find(selected_event);
      if(!selected || draw == draw_instances.end()) { error("Select a draw or mesh dispatch event first"); continue; }
      if(instance >= draw->second) { error("Mesh instance is out of range"); continue; }
      if(view >= replay.controller->GetPipelineState().MultiviewBroadcastCount())
      { error("Mesh view is out of range"); continue; }
      const GraphicsAPI api = replay.controller->GetAPIProperties().pipelineType;
      if(stage >= 3 && (api == GraphicsAPI::D3D11 || api == GraphicsAPI::OpenGL))
      { error("Task and mesh shader output is unavailable on this replay API"); continue; }
      mesh_ready = false;
      selected_mesh = replay.controller->GetPostVSData(uint32_t(instance), uint32_t(view), MeshDataStage(stage));
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      mesh_ready = true;
      mesh_instance = uint32_t(instance); mesh_view = uint32_t(view); mesh_stage = uint32_t(stage);
      const PipeState &pipeline = replay.controller->GetPipelineState();
      uint32_t restart_index = pipeline.GetRestartIndex();
      if(pipeline.GetIBuffer().byteStride == 1) restart_index &= 0xff;
      else if(pipeline.GetIBuffer().byteStride == 2) restart_index &= 0xffff;
      const bool restart_enabled = stage == uint32_t(MeshDataStage::VSOut) &&
                                   indexed_draws.count(selected_event) && pipeline.IsRestartEnabled();
      const ShaderReflection *task_shader = stage == uint32_t(MeshDataStage::TaskOut)
                                                ? pipeline.GetShaderReflection(ShaderStage::Task) : nullptr;
      write_mesh(selected_mesh, selected_event, uint32_t(instance), uint32_t(view), uint32_t(stage), restart_enabled, restart_index, task_shader, stage == uint32_t(MeshDataStage::TaskOut) && api == GraphicsAPI::Vulkan, pipeline);
      continue;
    }
    if(command.compare(0, 10, "mesh_read ") == 0)
    {
      std::istringstream arguments(command.substr(10));
      std::string instance_text, view_text, stage_text, kind, offset_text, length_text, extra;
      uint64_t instance = 0, view = 0, stage = 0, offset = 0, length = 0;
      if(!(arguments >> instance_text >> view_text >> stage_text >> kind >> offset_text >> length_text) || (arguments >> extra) ||
         !decimal(instance_text, instance) || !decimal(view_text, view) || !decimal(stage_text, stage) ||
         instance > UINT32_MAX || view > UINT32_MAX || stage < 1 || stage > 4 ||
         (kind != "vertex" && kind != "index") || !decimal(offset_text, offset) ||
         !decimal(length_text, length) || length > 65536)
      { error("Invalid mesh buffer request"); continue; }
      if(!selected || !mesh_ready) { error("Query mesh data for the selected event first"); continue; }
      if(instance != mesh_instance || view != mesh_view || stage != mesh_stage)
      { error("Mesh selection has changed"); continue; }
      const ResourceId id = kind == "vertex" ? selected_mesh.vertexResourceId : selected_mesh.indexResourceId;
      const uint64_t base = kind == "vertex" ? selected_mesh.vertexByteOffset : selected_mesh.indexByteOffset;
      if(id == ResourceId() || offset > UINT64_MAX - base || length > UINT64_MAX - (base + offset))
      { error("Mesh buffer is unavailable or range overflowed"); continue; }
      bytebuf data;
      if(length) data = replay.controller->GetBufferData(id, base + offset, length);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(data.size() > length) { error("Invalid mesh buffer response"); continue; }
      static const char hex[] = "0123456789abcdef";
      std::cout << "\nRENDERDOCX/1 {\"type\":\"mesh_data\",\"event_id\":" << selected_event << ",\"kind\":";
      json_string(kind.data(), kind.size()); std::cout << ",\"offset\":\"" << offset << "\",\"hex\":\"";
      for(byte value : data) std::cout << hex[value >> 4] << hex[value & 15];
      std::cout << "\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 16, "annotation_find ") == 0)
    {
      std::istringstream arguments(command.substr(16));
      std::string event_text, encoded;
      uint64_t event = 0;
      if(!(arguments >> event_text) || !decimal(event_text, event) || event > UINT32_MAX)
      { error("Invalid event annotation request"); continue; }
      auto found = event_annotations.find(uint32_t(event));
      if(found == event_annotations.end()) { error("Unknown event annotation"); continue; }
      std::vector<rdcstr> keys;
      bool valid = true;
      while(arguments >> encoded)
      {
        bytebuf bytes;
        if(!decode_section_hex(encoded, bytes)) { valid = false; break; }
        keys.push_back(bytes.empty() ? rdcstr() : rdcstr((const char *)bytes.data(), bytes.size()));
      }
      if(!valid || keys.empty() || keys.size() > 128) { error("Invalid event annotation keys"); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"annotation_find\",\"event_id\":" << event << ",\"matches\":[";
      for(size_t n = 0; n < keys.size(); ++n)
      {
        if(n) std::cout << ',';
        const SDObject *object = found->second ? found->second->FindChildByKeyPath(keys[n]) : nullptr;
        if(object) write_structured_node(object, resource_indices, &replacements);
        else std::cout << "null";
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 15, "structure_find ") == 0)
    {
      std::istringstream arguments(command.substr(15));
      std::string event_text, encoded;
      uint64_t event = 0;
      if(!(arguments >> event_text) || !decimal(event_text, event) || event > UINT32_MAX)
      { error("Invalid structured search event"); continue; }
      auto found = event_chunks.find(uint32_t(event));
      if(found == event_chunks.end()) { error("Structured data is unavailable for this event"); continue; }
      const SDObject *root = replay.controller->GetStructuredFile().chunks[found->second];
      std::vector<rdcstr> names;
      bool valid = true;
      while(arguments >> encoded)
      {
        bytebuf bytes;
        if(!decode_section_hex(encoded, bytes)) { valid = false; break; }
        names.push_back(bytes.empty() ? rdcstr() : rdcstr((const char *)bytes.data(), bytes.size()));
      }
      if(!valid || names.empty() || names.size() > 128) { error("Invalid structured search names"); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"structure_find\",\"event_id\":" << event << ",\"matches\":[";
      for(size_t n = 0; n < names.size(); ++n)
      {
        if(n) std::cout << ',';
        const SDObject *object = root->FindChildRecursively(names[n]);
        if(!object) { std::cout << "null"; continue; }
        std::vector<uint64_t> path;
        for(const SDObject *child = object; child != root; child = child->GetParent())
        {
          const SDObject *parent = child->GetParent();
          size_t index = 0;
          while(index < parent->NumChildren() && parent->GetChild(index) != child) ++index;
          path.push_back(uint64_t(index));
        }
        std::reverse(path.begin(), path.end());
        std::cout << "{\"path\":[";
        for(size_t i = 0; i < path.size(); ++i) { if(i) std::cout << ','; std::cout << '"' << path[i] << '"'; }
        std::cout << "],\"parent_flags\":" << uint32_t(object->GetParent()->type.flags) << ",\"node\":";
        write_structured_node(object, resource_indices, &replacements);
        std::cout << '}';
      }
      std::cout << "]}" << std::endl;
      continue;
    }
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
      const SDChunk *source_chunk = replay.controller->GetStructuredFile().chunks[found->second];
      std::cout << "],\"chunk_metadata\":{\"chunk_id\":" << source_chunk->metadata.chunkID
                << ",\"length\":\"" << source_chunk->metadata.length << "\",\"thread_id\":\""
                << source_chunk->metadata.threadID << "\"},\"node\":"; write_structured_node(object, resource_indices, &replacements);
      std::cout << ",\"children\":[";
      for(size_t i = 0; i < children.size(); ++i) { if(i) std::cout << ','; write_structured_node(children[i], resource_indices, &replacements); }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command == "custominfo")
    {
      const auto encodings = replay.controller->GetCustomShaderEncodings();
      const auto prefixes = replay.controller->GetCustomShaderSourcePrefixes();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"custom_shader_info\",\"active\":" << (custom_shader.active != ResourceId() ? "true" : "false") << ",\"encodings\":[";
      for(size_t i=0; i<encodings.size(); ++i) { if(i) std::cout << ','; std::cout << uint32_t(encodings[i]); }
      std::cout << "],\"prefixes\":[";
      for(size_t i=0; i<prefixes.size(); ++i) { if(i) std::cout << ','; std::cout << "{\"encoding\":" << uint32_t(prefixes[i].encoding) << ",\"prefix\":"; json_string(prefixes[i].prefix); std::cout << '}'; }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command == "customclear")
    {
      custom_shader.clear();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"custom_shader_build\",\"compiled\":false,\"active\":false,\"diagnostics\":\"\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 12, "custombuild ") == 0)
    {
      std::istringstream args(command.substr(12));
      std::string encoding_text, entry_text, source_text, prefix_text, count_text, extra;
      uint64_t encoding=0, use_prefix=0, count=0;
      bytebuf entry, source;
      if(!(args >> encoding_text >> entry_text >> source_text >> prefix_text >> count_text) ||
         !decimal(encoding_text,encoding) || encoding >= uint32_t(ShaderEncoding::Count) ||
         !decode_section_hex(entry_text,entry) || entry.empty() || std::find(entry.begin(),entry.end(),byte(0)) != entry.end() ||
         !decode_section_hex(source_text,source) || source.empty() || !decimal(prefix_text,use_prefix) || use_prefix > 1 ||
         !decimal(count_text,count) || count > command.size() / 4)
      { error("Invalid custom shader request"); continue; }
      ShaderCompileFlags flags; bool valid=true;
      for(uint64_t i=0; i<count; ++i)
      {
        std::string name, value; bytebuf n,v;
        if(!(args >> name >> value) || !decode_section_hex(name,n) || !decode_section_hex(value,v)) { valid=false; break; }
        flags.flags.push_back({n.empty() ? rdcstr() : rdcstr((const char *)n.data(),n.size()), v.empty() ? rdcstr() : rdcstr((const char *)v.data(),v.size())});
      }
      if(!valid || (args >> extra)) { error("Invalid custom shader flags"); continue; }
      const auto encodings = replay.controller->GetCustomShaderEncodings();
      if(std::find(encodings.begin(),encodings.end(),ShaderEncoding(encoding)) == encodings.end()) { error("Custom shader encoding is not supported"); continue; }
      if(use_prefix)
      {
        const auto prefixes = replay.controller->GetCustomShaderSourcePrefixes();
        for(const auto &prefix : prefixes) if(prefix.encoding == ShaderEncoding(encoding))
        {
          bytebuf combined; combined.append((const byte *)prefix.prefix.data(),prefix.prefix.size()); combined.append(source); source.swap(combined); break;
        }
      }
      auto built = replay.controller->BuildCustomShader(rdcstr((const char *)entry.data(),entry.size()), ShaderEncoding(encoding), source, flags, ShaderStage::Pixel);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { if(built.first != ResourceId()) replay.controller->FreeCustomShader(built.first); error(result_message(result)); break; }
      const bool compiled = built.first != ResourceId();
      if(compiled) { custom_shader.clear(); custom_shader.active = built.first; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"custom_shader_build\",\"compiled\":" << (compiled ? "true" : "false") << ",\"active\":" << (custom_shader.active != ResourceId() ? "true" : "false") << ",\"diagnostics\":";
      json_string(built.second); std::cout << "}" << std::endl;
      continue;
    }
    if(command.compare(0, 13, "shader_apply ") == 0 || command.compare(0, 14, "shader_revert ") == 0)
    {
      const bool applying = command.compare(0, 13, "shader_apply ") == 0;
      std::istringstream args(command.substr(applying ? 13 : 14));
      std::string event_text, stage_text, encoding_text, entry_text, source_text, extra;
      uint64_t requested_event = 0, stage_index = 0, encoding = 0;
      if(!(args >> event_text >> stage_text) || !decimal(event_text, requested_event) || requested_event > UINT32_MAX || !decimal(stage_text, stage_index) || stage_index >= 8 ||
         (applying && (!(args >> encoding_text >> entry_text >> source_text) || !decimal(encoding_text, encoding) || encoding >= uint32_t(ShaderEncoding::Count))))
      { error("Invalid shader replacement request"); continue; }
      ShaderCompileFlags supplied_flags;
      bool has_flags = false, flags_valid = true;
      if(applying && (args >> extra))
      {
        uint64_t count = 0;
        has_flags = true;
        if(!decimal(extra, count) || count > command.size() / 4) flags_valid = false;
        for(uint64_t i = 0; flags_valid && i < count; ++i)
        {
          std::string name, value; bytebuf name_bytes, value_bytes;
          if(!(args >> name >> value) || !decode_section_hex(name, name_bytes) || !decode_section_hex(value, value_bytes)) { flags_valid = false; break; }
          const rdcstr flag_name = name_bytes.empty() ? rdcstr() : rdcstr((const char *)name_bytes.data(), name_bytes.size());
          const rdcstr flag_value = value_bytes.empty() ? rdcstr() : rdcstr((const char *)value_bytes.data(), value_bytes.size());
          supplied_flags.flags.push_back({flag_name, flag_value});
        }
      }
      if(!flags_valid || (args >> extra)) { error("Invalid shader compile flags"); continue; }
      if(!selected) { error("Select an event before editing a shader"); continue; }
      if(requested_event != selected_event) { error("Shader edit event changed"); continue; }
      const ShaderStage stage = ShaderStage(stage_index);
      const PipeState &state = replay.controller->GetPipelineState();
      const ResourceId from = replacements.original(state.GetShader(stage));
      if(from == ResourceId()) { error("No shader is bound to this stage"); continue; }
      rdcstr diagnostics;
      bool compiled = false;
      if(applying)
      {
        bytebuf entry_bytes, source_bytes;
        if(!decode_section_hex(entry_text, entry_bytes) || entry_bytes.empty() ||
           !decode_section_hex(source_text, source_bytes) || source_bytes.empty() ||
           std::find(entry_bytes.begin(), entry_bytes.end(), byte(0)) != entry_bytes.end())
        { error("Invalid shader entry point or source bytes"); continue; }
        const auto encodings = replay.controller->GetTargetShaderEncodings();
        if(std::find(encodings.begin(), encodings.end(), ShaderEncoding(encoding)) == encodings.end())
        { error("Shader encoding is not supported by the replay device"); continue; }
        const ShaderReflection *reflection = state.GetShaderReflection(stage);
        if(!reflection) { error("Shader reflection is unavailable"); continue; }
        ShaderCompileFlags flags = has_flags ? supplied_flags : reflection->debugInfo.compileFlags;
        const rdcstr entry((const char *)entry_bytes.data(), entry_bytes.size());
        auto built = replay.controller->BuildTargetShader(entry, ShaderEncoding(encoding), source_bytes, flags, stage);
        diagnostics = built.second;
        compiled = built.first != ResourceId();
        replacements.remove(from);
        if(compiled)
        {
          replay.controller->ReplaceResource(from, built.first);
          replacements.replacements[from] = built.first;
        }
      }
      else replacements.remove(from);
      raw_ready = false; raw_cache = bytebuf();
      replay.controller->ClearReplayCache();
      replay.controller->SetFrameEvent(selected_event, true);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      mesh_ready = false; history_ready = false; cached_history = rdcarray<PixelModification>();
      counters_ready = false; counter_results = rdcarray<CounterResult>();
      std::cout << "\nRENDERDOCX/1 {\"type\":\"shader_edit\",\"event_id\":" << selected_event
                << ",\"stage\":" << stage_index << ",\"applied\":" << (compiled ? "true" : "false") << ",\"diagnostics\":";
      json_string(diagnostics); std::cout << ",\"pipeline\":"; write_pipeline(replay.controller, selected_event); std::cout << "}" << std::endl;
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
      std::cout << ",\"dispatch_threads\":";
      if(stage == ShaderStage::Compute)
        std::cout << '[' << reflection->dispatchThreadsDimension[0] << ',' << reflection->dispatchThreadsDimension[1] << ',' << reflection->dispatchThreadsDimension[2] << ']';
      else std::cout << "null";
      std::cout << ",\"debuggable\":" << (reflection->debugInfo.debuggable ? "true" : "false")
                << ",\"source_debug_info\":" << (reflection->debugInfo.sourceDebugInformation ? "true" : "false")
                << ",\"debug_status\":";
      json_string(reflection->debugInfo.debugStatus);
      std::cout << ",\"source_encoding\":" << uint32_t(reflection->debugInfo.encoding) << ",\"source_entry\":";
      json_string(reflection->debugInfo.entrySourceName);
      std::cout << ",\"compile_flags\":[";
      for(size_t i = 0; i < reflection->debugInfo.compileFlags.flags.size(); ++i)
      {
        if(i) std::cout << ',';
        const ShaderCompileFlag &flag = reflection->debugInfo.compileFlags.flags[i];
        std::cout << "{\"name\":"; json_string(flag.name);
        std::cout << ",\"value\":"; json_string(flag.value); std::cout << '}';
      }
      std::cout << ']';
      std::cout << ",\"target_encodings\":[";
      const auto encodings = replay.controller->GetTargetShaderEncodings();
      for(size_t i = 0; i < encodings.size(); ++i) { if(i) std::cout << ','; std::cout << uint32_t(encodings[i]); }
      std::cout << ']';
      std::cout << ",\"source_files\":[";
      for(size_t i = 0; i < reflection->debugInfo.files.size(); ++i)
      {
        if(i) std::cout << ',';
        std::cout << "{\"filename\":"; json_string(reflection->debugInfo.files[i].filename);
        std::cout << ",\"contents\":"; json_string(reflection->debugInfo.files[i].contents);
        std::cout << '}';
      }
      std::cout << ']';
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
                << ",\"samplers\":" << reflection->samplers.size() << ",\"constant_block_details\":[";
      for(size_t i = 0; i < reflection->constantBlocks.size(); ++i)
      {
        if(i) std::cout << ',';
        const ConstantBlock &block = reflection->constantBlocks[i];
        std::cout << "{\"name\":"; json_string(block.name);
        std::cout << ",\"bind\":" << block.fixedBindNumber << ",\"space\":" << block.fixedBindSetOrSpace
                  << ",\"array_size\":" << block.bindArraySize << ",\"byte_size\":" << block.byteSize
                  << ",\"buffer_backed\":" << (block.bufferBacked ? "true" : "false")
                  << ",\"inline_data\":" << (block.inlineDataBytes ? "true" : "false")
                  << ",\"compile_constants\":" << (block.compileConstants ? "true" : "false") << ",\"variables\":[";
        for(size_t j = 0; j < block.variables.size(); ++j) { if(j) std::cout << ','; write_reflected_constant(block.variables[j]); }
        std::cout << "]}";
      }
      std::cout << "],\"sampler_details\":[";
      for(size_t i = 0; i < reflection->samplers.size(); ++i)
      {
        if(i) std::cout << ',';
        const ShaderSampler &sampler = reflection->samplers[i];
        std::cout << "{\"name\":"; json_string(sampler.name);
        std::cout << ",\"bind\":" << sampler.fixedBindNumber << ",\"space\":" << sampler.fixedBindSetOrSpace
                  << ",\"array_size\":" << sampler.bindArraySize << '}';
      }
      auto resource_details = [](const rdcarray<ShaderResource> &resources) {
        std::cout << '[';
        for(size_t i = 0; i < resources.size(); ++i)
        {
          if(i) std::cout << ',';
          const ShaderResource &r = resources[i];
          std::cout << "{\"name\":"; json_string(r.name);
          std::cout << ",\"bind\":" << r.fixedBindNumber << ",\"space\":" << r.fixedBindSetOrSpace
                    << ",\"array_size\":" << r.bindArraySize << ",\"texture_type\":" << uint32_t(r.textureType)
                    << ",\"descriptor_type\":" << uint32_t(r.descriptorType) << ",\"is_texture\":" << (r.isTexture ? "true" : "false")
                    << ",\"has_sampler\":" << (r.hasSampler ? "true" : "false") << ",\"input_attachment\":" << (r.isInputAttachment ? "true" : "false")
                    << ",\"read_only\":" << (r.isReadOnly ? "true" : "false") << ",\"variable_type\":";
          write_reflected_type(r.variableType); std::cout << '}';
        }
        std::cout << ']';
      };
      std::cout << "],\"read_only_details\":"; resource_details(reflection->readOnlyResources);
      std::cout << ",\"read_write_details\":"; resource_details(reflection->readWriteResources);
      std::cout << "}" << std::endl;
      continue;
    }
    if(command.compare(0, 9, "bindings ") == 0)
    {
      std::istringstream arguments(command.substr(9));
      std::string stage_text, kind, offset_text, limit_text, extra;
      uint64_t stage_index = 0, offset = 0, limit = 0;
      if(!(arguments >> stage_text >> kind >> offset_text >> limit_text) || (arguments >> extra) ||
         !decimal(stage_text, stage_index) || !decimal(offset_text, offset) || !decimal(limit_text, limit) ||
         stage_index >= 8 || limit == 0 || limit > 128 ||
         (kind != "constants" && kind != "read_only" && kind != "read_write" && kind != "samplers"))
      { error("Invalid shader bindings request"); continue; }
      if(!selected) { error("Select an event first"); continue; }
      const PipeState &state = replay.controller->GetPipelineState();
      const ShaderStage stage = ShaderStage(stage_index);
      if(state.GetShader(stage) == ResourceId()) { error("No shader is bound to this stage"); continue; }
      rdcarray<UsedDescriptor> bindings;
      if(kind == "constants") bindings = state.GetConstantBlocks(stage);
      else if(kind == "read_only") bindings = state.GetReadOnlyResources(stage);
      else if(kind == "read_write") bindings = state.GetReadWriteResources(stage);
      else bindings = state.GetSamplers(stage);
      if(offset > bindings.size()) { error("Shader binding offset is out of range"); continue; }
      const size_t start = size_t(offset), end = start + (std::min)(size_t(limit), bindings.size() - start);
      std::cout << "\nRENDERDOCX/1 {\"type\":\"shader_bindings\",\"event_id\":" << selected_event
                << ",\"stage\":" << stage_index << ",\"kind\":"; json_string(kind.data(), kind.size());
      std::cout << ",\"offset\":\"" << offset << "\",\"total\":\"" << bindings.size() << "\",\"entries\":[";
      for(size_t i = start; i < end; ++i)
      {
        if(i != start) std::cout << ',';
        const UsedDescriptor &binding = bindings[i];
        const auto &access = binding.access;
        const auto store = resource_indices.find(access.descriptorStore);
        std::cout << "{\"binding\":" << access.index << ",\"array_element\":" << access.arrayElement
                  << ",\"type\":" << uint32_t(access.type) << ",\"statically_unused\":" << (access.staticallyUnused ? "true" : "false")
                  << ",\"store_resource\":";
        if(store == resource_indices.end() || access.descriptorStore == ResourceId()) std::cout << "null"; else std::cout << store->second;
        std::cout << ",\"store_offset\":" << access.byteOffset << ",\"store_size\":" << access.byteSize << ",\"descriptor\":";
        if(kind == "samplers") std::cout << "null"; else write_descriptor(binding.descriptor, resource_indices);
        std::cout << ",\"sampler\":";
        if(kind == "samplers" || kind == "read_only") write_sampler(binding.sampler, resource_indices); else std::cout << "null";
        std::cout << '}';
      }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 7, "consts ") == 0)
    {
      std::istringstream arguments(command.substr(7));
      std::string stage_text, block_text, element_text, extra;
      uint64_t stage_index = 0, block_index = 0, element = 0;
      if(!(arguments >> stage_text >> block_text >> element_text) || (arguments >> extra) ||
         !decimal(stage_text, stage_index) || !decimal(block_text, block_index) || !decimal(element_text, element) ||
         stage_index >= 8 || block_index > UINT32_MAX || element > UINT32_MAX)
      { error("Invalid constant block request"); continue; }
      if(!selected) { error("Select an event first"); continue; }
      const PipeState &state = replay.controller->GetPipelineState();
      const ShaderStage stage = ShaderStage(stage_index);
      const ShaderReflection *reflection = state.GetShaderReflection(stage);
      if(!reflection || block_index >= reflection->constantBlocks.size()) { error("Constant block is unavailable"); continue; }
      const ConstantBlock &block = reflection->constantBlocks[size_t(block_index)];
      if(element >= block.bindArraySize) { error("Constant block array element is out of range"); continue; }
      const UsedDescriptor binding = state.GetConstantBlock(stage, uint32_t(block_index), uint32_t(element));
      if(block.bufferBacked && !block.inlineDataBytes && binding.descriptor.resource == ResourceId())
      { error("Constant buffer is unbound"); continue; }
      const ResourceId pipeline = stage == ShaderStage::Compute ? state.GetComputePipelineObject() : state.GetGraphicsPipelineObject();
      const auto variables = replay.controller->GetCBufferVariableContents(pipeline, state.GetShader(stage), stage,
          state.GetShaderEntryPoint(stage), uint32_t(block_index), binding.descriptor.resource,
          binding.descriptor.byteOffset, binding.descriptor.byteSize);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"shader_constants\",\"event_id\":" << selected_event
                << ",\"stage\":" << stage_index << ",\"block\":" << block_index << ",\"element\":" << element
                << ",\"byte_offset\":\"" << binding.descriptor.byteOffset << "\",\"byte_size\":\"" << binding.descriptor.byteSize
                << "\",\"variables\":"; write_debug_variables(variables); std::cout << '}' << std::endl;
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
    if(command.compare(0, 8, "history ") == 0)
    {
      if(!selected) { error("Select an event before inspecting pixel history"); continue; }
      std::istringstream arguments(command.substr(8));
      std::string index_text, mip_text, slice_text, sample_text, x_text, y_text, offset_text, limit_text, cast_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, x = 0, y = 0, offset = 0, limit = 0, cast = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> x_text >> y_text >> offset_text >> limit_text) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) || !decimal(x_text, x) || !decimal(y_text, y) ||
         !decimal(offset_text, offset) || !decimal(limit_text, limit) || index >= texture_ids.size() || limit == 0 || limit > 128)
      { error("Invalid pixel history request"); continue; }
      if((arguments >> cast_text) && (!decimal(cast_text, cast) || cast > 9 || (arguments >> extra)))
      { error("Invalid texture cast"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      if(mip >= texture.mips || mip >= 32 || !texture.width || !texture.height || !texture.samples)
      { error("Invalid texture dimensions, mip or sample count"); continue; }
      const uint32_t width = (std::max)(1U, texture.width >> uint32_t(mip));
      const uint32_t height = (std::max)(1U, texture.height >> uint32_t(mip));
      const uint32_t depth = texture.dimension == 3 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : texture.arraysize;
      if(x >= width || y >= height || slice >= depth || (sample_text != "resolve" && sample >= texture.samples))
      { error("Pixel history selection is out of range"); continue; }
      const uint32_t actual_sample = sample_text == "resolve" ? (texture.samples > 1 ? UINT32_MAX : 0U) : uint32_t(sample);
      mesh_ready = false;
      if(!history_ready || history_event != selected_event || history_index != index || history_mip != mip || history_slice != slice ||
         history_sample != actual_sample || history_x != x || history_y != y || history_cast != cast)
      {
        history_ready = false;
        cached_history = rdcarray<PixelModification>();
        cached_history = replay.controller->PixelHistory(texture.id, uint32_t(x), uint32_t(y), Subresource(uint32_t(mip), uint32_t(slice), actual_sample), CompType(cast));
        result = replay.controller->GetFatalErrorStatus();
        if(!result.OK()) { error(result_message(result)); break; }
        history_event = selected_event; history_index = uint32_t(index); history_mip = uint32_t(mip); history_slice = uint32_t(slice);
        history_sample = actual_sample; history_x = uint32_t(x); history_y = uint32_t(y); history_cast = uint32_t(cast); history_ready = true;
      }
      if(offset > cached_history.size()) { error("Pixel history offset is out of range"); continue; }
      const size_t start = size_t(offset), end = start + (std::min)(size_t(limit), cached_history.size() - start);
      std::cout << "\nRENDERDOCX/1 {\"type\":\"pixel_history\",\"event_id\":" << selected_event << ",\"index\":" << index
                << ",\"mip\":" << mip << ",\"slice\":" << slice << ",\"sample\":" << actual_sample << ",\"x\":" << x << ",\"y\":" << y
                << ",\"offset\":\"" << offset << "\",\"total\":\"" << cached_history.size() << "\",\"entries\":[";
      for(size_t i = start; i < end; ++i) { if(i != start) std::cout << ','; write_pixel_modification(cached_history[i]); }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 6, "stats ") == 0 || command.compare(0, 12, "customstats ") == 0 ||
       command.compare(0, 7, "minmax ") == 0 || command.compare(0, 13, "customminmax ") == 0)
    {
      const bool minmax_only = command.compare(0, 7, "minmax ") == 0 || command.compare(0, 13, "customminmax ") == 0;
      const bool custom_stats = command.compare(0, 12, "customstats ") == 0 || command.compare(0, 13, "customminmax ") == 0;
      if(!selected) { error("Select an event before computing texture statistics"); continue; }
      std::istringstream arguments(command.substr(minmax_only ? (custom_stats ? 13 : 7) : (custom_stats ? 12 : 6)));
      std::string index_text, mip_text, slice_text, sample_text, min_text, max_text, mask_text, cast_text, display_cast_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, min_bits = 0, max_bits = 0, mask = 0, cast = 0, display_cast = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> min_text >> max_text >> mask_text) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) || !decimal(min_text, min_bits) || !decimal(max_text, max_bits) || !decimal(mask_text, mask) ||
         index >= texture_ids.size() || min_bits > UINT32_MAX || max_bits > UINT32_MAX || mask > 15)
      { error("Invalid texture statistics request"); continue; }
      if(custom_stats)
      {
        if(!(arguments >> cast_text >> display_cast_text) || !decimal(cast_text, cast) || cast > 9 || !decimal(display_cast_text, display_cast) || display_cast > 9 || (arguments >> extra))
        { error("Invalid custom texture statistics request"); continue; }
        if(custom_shader.active == ResourceId()) { error("Custom shader is unavailable"); continue; }
      }
      else if((arguments >> cast_text) && (!decimal(cast_text, cast) || cast > 9 || (arguments >> extra)))
      { error("Invalid texture cast"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      if(mip >= texture.mips || mip >= 32 || !texture.width || !texture.height || !texture.samples)
      { error("Invalid texture dimensions, mip or sample count"); continue; }
      const uint32_t depth = texture.dimension == 3 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : texture.arraysize;
      if(slice >= depth || (sample_text != "resolve" && sample >= texture.samples))
      { error("Texture selection is out of range"); continue; }
      const uint32_t lo_bits = uint32_t(min_bits), hi_bits = uint32_t(max_bits);
      float lo = 0, hi = 0;
      std::memcpy(&lo, &lo_bits, sizeof(lo)); std::memcpy(&hi, &hi_bits, sizeof(hi));
      if(!std::isfinite(lo) || !std::isfinite(hi) || !(lo < hi) || !std::isfinite(hi - lo) || !std::isfinite(hi + hi * 1e-6f))
      { error("Invalid histogram range"); continue; }
      const uint32_t actual_sample = sample_text == "resolve" ? (texture.samples > 1 ? UINT32_MAX : 0U) : uint32_t(sample);
      Subresource sub(uint32_t(mip), uint32_t(slice), actual_sample);
      ResourceId stats_resource = texture.id;
      CompType stats_cast = CompType(cast);
      ScopedReplayOutput scoped;
      if(custom_stats)
      {
        scoped.output = replay.controller->CreateOutput(CreateHeadlessWindowingData(1, 1), ReplayOutputType::Texture);
        if(!scoped.output) { error("Cannot create a headless texture output"); continue; }
        TextureDisplay display; display.resourceId=texture.id; display.typeCast=CompType(display_cast); display.subresource=sub; display.customShaderId=custom_shader.active;
        scoped.output->SetTextureDisplay(display);
        stats_resource = scoped.output->GetCustomShaderTexID();
        if(stats_resource == ResourceId()) { error("Custom shader output is unavailable"); continue; }
        stats_cast = CompType::Typeless; sub.slice=0; sub.sample=0;
      }
      const auto extrema = replay.controller->GetMinMax(stats_resource, sub, stats_cast);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      const rdcfixedarray<bool, 4> channels = {(mask & 1) != 0, (mask & 2) != 0, (mask & 4) != 0, (mask & 8) != 0};
      rdcarray<uint32_t> histogram;
      if(!minmax_only)
      {
        histogram = replay.controller->GetHistogram(stats_resource, sub, stats_cast, lo, hi, channels);
        result = replay.controller->GetFatalErrorStatus();
        if(!result.OK()) { error(result_message(result)); break; }
      }
      uint32_t low[4], high[4]; std::memcpy(low, &extrema.first, sizeof(low)); std::memcpy(high, &extrema.second, sizeof(high));
      std::cout << "\nRENDERDOCX/1 {\"type\":\"texture_stats\",\"event_id\":" << selected_event << ",\"index\":" << index
                << ",\"custom\":" << (custom_stats ? "true" : "false") << ",\"mip\":" << mip << ",\"slice\":" << slice << ",\"sample\":" << actual_sample
                << ",\"range_min_bits\":" << lo_bits << ",\"range_max_bits\":" << hi_bits << ",\"channel_mask\":" << mask
                << ",\"min_bits\":[" << low[0] << ',' << low[1] << ',' << low[2] << ',' << low[3] << "],\"max_bits\":[" << high[0] << ',' << high[1] << ',' << high[2] << ',' << high[3] << "],\"histogram\":[";
      for(size_t i = 0; i < histogram.size(); ++i) { if(i) std::cout << ','; std::cout << histogram[i]; }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 6, "pixel ") == 0 || command.compare(0, 13, "overlaypixel ") == 0 || command.compare(0, 12, "custompixel ") == 0)
    {
      const bool overlay_request = command.compare(0, 13, "overlaypixel ") == 0;
      const bool custom_request = command.compare(0, 12, "custompixel ") == 0;
      if(!selected) { error("Select an event before picking a pixel"); continue; }
      std::istringstream arguments(command.substr(overlay_request ? 13 : custom_request ? 12 : 6));
      std::string index_text, mip_text, slice_text, sample_text, x_text, y_text, cast_text, overlay_text, display_cast_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, x = 0, y = 0, cast = 0, overlay = 0, display_cast = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> x_text >> y_text) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) || !decimal(x_text, x) || !decimal(y_text, y) || index >= texture_ids.size())
      { error("Invalid pixel request"); continue; }
      if(overlay_request || custom_request)
      {
        if(!(arguments >> cast_text >> overlay_text >> display_cast_text) || !decimal(cast_text,cast) || cast > 9 ||
           !decimal(overlay_text,overlay) || overlay > 14 || (overlay_request && overlay < 11) || !decimal(display_cast_text,display_cast) || display_cast > 9 || (arguments >> extra))
        { error("Invalid overlay pixel request"); continue; }
      }
      else if((arguments >> cast_text) && (!decimal(cast_text, cast) || cast > 9 || (arguments >> extra)))
      { error("Invalid texture cast"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      if(mip >= texture.mips || mip >= 32 || !texture.width || !texture.height || !texture.samples)
      { error("Invalid texture dimensions, mip or sample count"); continue; }
      const uint32_t width = (std::max)(1U, texture.width >> uint32_t(mip));
      const uint32_t height = (std::max)(1U, texture.height >> uint32_t(mip));
      const uint32_t depth = texture.dimension == 3 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : texture.arraysize;
      if(x >= width || y >= height || slice >= depth || (sample_text != "resolve" && sample >= texture.samples))
      { error("Pixel selection is out of range"); continue; }
      const uint32_t actual_sample = sample_text == "resolve" ? (texture.samples > 1 ? UINT32_MAX : 0U) : uint32_t(sample);
      if(custom_request && custom_shader.active == ResourceId()) { error("Custom shader is unavailable"); continue; }
      const bool overlay_replay = (overlay_request || custom_request) && overlay >= 11 && overlay <= 14;
      ResourceId pixel_resource = texture.id;
      CompType pixel_cast = CompType(cast);
      bool overlay_source = false;
      IReplayOutput *output = nullptr;
      if(overlay_request || custom_request)
      {
        output = replay.controller->CreateOutput(CreateHeadlessWindowingData(1, 1), ReplayOutputType::Texture);
        if(!output) { error("Cannot create a headless texture output"); continue; }
        TextureDisplay display;
        display.resourceId = texture.id;
        display.typeCast = CompType(display_cast);
        display.subresource = Subresource(uint32_t(mip), uint32_t(slice), actual_sample);
        display.overlay = DebugOverlay(overlay);
        display.customShaderId = custom_request ? custom_shader.active : ResourceId();
        output->SetTextureDisplay(display);
        if(overlay_replay)
        {
          ResourceId overlay_id = output->GetDebugOverlayTexID();
          if(overlay_id != ResourceId()) { pixel_resource=overlay_id; pixel_cast=CompType::Typeless; overlay_source=true; }
        }
      }
      PixelValue pixel = replay.controller->PickPixel(pixel_resource, uint32_t(x), uint32_t(y), Subresource(uint32_t(mip), uint32_t(slice), actual_sample), pixel_cast);
      PixelValue custom_pixel = {};
      bool custom_available = false;
      if(custom_request)
      {
        ResourceId custom_id = output->GetCustomShaderTexID();
        if(custom_id != ResourceId())
        {
          custom_pixel = replay.controller->PickPixel(custom_id, uint32_t(x), uint32_t(y), Subresource(uint32_t(mip), 0, 0), CompType::Typeless);
          custom_available = true;
        }
      }
      if(output) output->Shutdown();
      if(overlay_replay)
      {
        replay.controller->SetFrameEvent(selected_event, true);
        mesh_ready = false; history_ready = false; raw_ready = false; counters_ready = false;
        cached_history = rdcarray<PixelModification>(); raw_cache = bytebuf(); counter_results = rdcarray<CounterResult>();
      }
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(custom_request && !custom_available) { error("Custom shader output is unavailable"); continue; }
      uint32_t bits[4]; std::memcpy(bits, &pixel, sizeof(bits));
      uint32_t custom_bits[4]; std::memcpy(custom_bits, &custom_pixel, sizeof(custom_bits));
      std::cout << "\nRENDERDOCX/1 {\"type\":\"pixel\",\"event_id\":" << selected_event << ",\"index\":" << index
                << ",\"mip\":" << mip << ",\"slice\":" << slice << ",\"sample\":" << actual_sample
                << ",\"x\":" << x << ",\"y\":" << y << ",\"overlay\":" << (overlay_source ? "true" : "false") << ",\"bits\":[" << bits[0] << ',' << bits[1] << ',' << bits[2] << ',' << bits[3] << "]";
      if(custom_request) std::cout << ",\"custom_bits\":[" << custom_bits[0] << ',' << custom_bits[1] << ',' << custom_bits[2] << ',' << custom_bits[3] << ']';
      std::cout << "}" << std::endl;
      continue;
    }
    if(command.compare(0, 8, "rawpage ") == 0)
    {
      if(!selected) { error("Select an event before reading textures"); continue; }
      std::istringstream args(command.substr(8));
      std::string index_text, mip_text, slice_text, sample_text, offset_text, length_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, offset = 0, length = 0;
      if(!(args >> index_text >> mip_text >> slice_text >> sample_text >> offset_text >> length_text) || (args >> extra) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         !decimal(sample_text, sample) || !decimal(offset_text, offset) || !decimal(length_text, length) ||
         index >= texture_ids.size() || length > 65536)
      { error("Invalid raw texture page request"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      if(mip >= texture.mips || slice >= texture.arraysize || sample >= texture.samples ||
         (texture.dimension == 3 && slice != 0))
      { error("Raw texture selection is out of range"); continue; }
      if(!raw_ready || raw_event != selected_event || raw_index != index || raw_mip != mip || raw_slice != slice || raw_sample != sample)
      {
        raw_ready = false;
        raw_cache = bytebuf();
        raw_cache = replay.controller->GetTextureData(texture.id, Subresource(uint32_t(mip), uint32_t(slice), uint32_t(sample)));
        result = replay.controller->GetFatalErrorStatus();
        if(!result.OK()) { error(result_message(result)); break; }
        if(raw_cache.empty()) { error("Texture data is unavailable"); continue; }
        raw_ready = true; raw_event = selected_event; raw_index = uint32_t(index); raw_mip = uint32_t(mip);
        raw_slice = uint32_t(slice); raw_sample = uint32_t(sample);
      }
      if(offset > raw_cache.size()) { error("Raw texture offset is out of range"); continue; }
      const uint64_t count = (std::min)(length, uint64_t(raw_cache.size()) - offset);
      static const char hex[] = "0123456789abcdef";
      std::cout << "\nRENDERDOCX/1 {\"type\":\"raw_texture_page\",\"event_id\":" << selected_event
                << ",\"index\":" << index << ",\"mip\":" << mip << ",\"slice\":" << slice << ",\"sample\":" << sample
                << ",\"offset\":\"" << offset << "\",\"total\":\"" << raw_cache.size() << "\",\"hex\":\"";
      for(uint64_t i = 0; i < count; ++i) { const byte b = raw_cache[size_t(offset + i)]; std::cout << hex[b >> 4] << hex[b & 15]; }
      std::cout << "\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 8, "rawsave ") == 0)
    {
      if(!selected) { error("Select an event before exporting textures"); continue; }
      std::istringstream arguments(command.substr(8));
      std::string index_text, mip_text, slice_text, sample_text, encoded, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> encoded) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         !decimal(sample_text, sample) || index >= texture_ids.size())
      { error("Invalid raw texture selection"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      if(mip >= texture.mips || slice >= texture.arraysize || sample >= texture.samples ||
         (texture.dimension == 3 && slice != 0))
      { error("Raw texture selection is out of range"); continue; }
      bytebuf path_bytes;
      if(!decode_section_hex(encoded, path_bytes) || path_bytes.empty() ||
         std::find(path_bytes.begin(), path_bytes.end(), byte(0)) != path_bytes.end())
      { error("Invalid output path"); continue; }
      const std::string path(path_bytes.begin(), path_bytes.end());
      const bytebuf bytes = replay.controller->GetTextureData(texture.id, Subresource(uint32_t(mip), uint32_t(slice), uint32_t(sample)));
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(bytes.empty()) { error("Texture data is unavailable"); continue; }
      try
      {
        std::ofstream file(std::filesystem::u8path(path), std::ios::binary | std::ios::trunc);
        if(!file) { error("Cannot open raw texture output"); continue; }
        size_t cursor = 0;
        while(cursor < bytes.size())
        {
          const size_t count = (std::min)(bytes.size() - cursor, size_t(16U * 1024U * 1024U));
          file.write(reinterpret_cast<const char *>(bytes.data() + cursor), std::streamsize(count));
          if(!file) break;
          cursor += count;
        }
        file.close();
        if(!file) { error("Cannot write raw texture output"); continue; }
      }
      catch(const std::exception &) { error("Cannot write raw texture output"); continue; }
      std::cout << "\nRENDERDOCX/1 {\"type\":\"texture_saved\",\"index\":" << index << "}" << std::endl;
      continue;
    }
    if(command.compare(0, 15, "render_texture ") == 0)
    {
      if(!selected) { error("Select an event before rendering textures"); continue; }
      std::istringstream arguments(command.substr(15));
      std::string i_text, mip_text, slice_text, sample_text, channel_text, cast_text, width_text, height_text, black_text, white_text, mask_text, flip_text, gamma_text, overlay_text, scale_text, x_text, y_text, hdr_text, yuv_text, background_text, custom_text, extra;
      uint64_t index=0, mip=0, slice=0, sample=0, channel=0, cast=0, width=0, height=0, black_bits=0, white_bits=0;
      if(!(arguments >> i_text >> mip_text >> slice_text >> sample_text >> channel_text >> cast_text >> width_text >> height_text >> black_text >> white_text) ||
         !decimal(i_text,index) || index >= texture_ids.size() || !decimal(mip_text,mip) || !decimal(slice_text,slice) ||
         (sample_text != "resolve" && !decimal(sample_text,sample)) || (channel_text != "all" && !decimal(channel_text,channel)) ||
         !decimal(cast_text,cast) || cast > 9 || !decimal(width_text,width) || !decimal(height_text,height) ||
         width == 0 || height == 0 || width > INT32_MAX || height > INT32_MAX || width * height > UINT32_MAX / 3 ||
         !decimal(black_text,black_bits) || black_bits > UINT32_MAX || !decimal(white_text,white_bits) || white_bits > UINT32_MAX)
      { error("Invalid texture render request"); continue; }
      uint64_t mask=0, flip=0, gamma=1, overlay=0;
      const bool has_options = bool(arguments >> mask_text);
      if(has_options && (!(arguments >> flip_text >> gamma_text) || !decimal(mask_text,mask) || mask > 15 ||
                         !decimal(flip_text,flip) || flip > 1 || !decimal(gamma_text,gamma) || gamma > 1))
      { error("Invalid texture display options"); continue; }
      if(has_options && (arguments >> overlay_text) && (!decimal(overlay_text,overlay) || overlay > 14))
      { error("Invalid texture overlay"); continue; }
      float scale = -1.0f, x_offset = 0.0f, y_offset = 0.0f;
      if(arguments >> scale_text)
      {
        uint64_t scale_bits=0, x_bits=0, y_bits=0;
        if(!(arguments >> x_text >> y_text) || !decimal(scale_text,scale_bits) || scale_bits > UINT32_MAX ||
           !decimal(x_text,x_bits) || x_bits > UINT32_MAX || !decimal(y_text,y_bits) || y_bits > UINT32_MAX)
        { error("Invalid texture view transform"); continue; }
        const uint32_t sb=uint32_t(scale_bits), xb=uint32_t(x_bits), yb=uint32_t(y_bits);
        std::memcpy(&scale,&sb,sizeof(scale)); std::memcpy(&x_offset,&xb,sizeof(x_offset)); std::memcpy(&y_offset,&yb,sizeof(y_offset));
        if(!std::isfinite(scale) || (scale <= 0.0f && scale != -1.0f) || !std::isfinite(x_offset) || !std::isfinite(y_offset) || (scale == -1.0f && (x_offset != 0.0f || y_offset != 0.0f)))
        { error("Invalid texture view transform"); continue; }
      }
      float hdr_multiplier = -1.0f;
      uint64_t yuv = 0;
      if(arguments >> hdr_text)
      {
        uint64_t hdr_bits=0;
        if(!(arguments >> yuv_text) || !decimal(hdr_text,hdr_bits) || hdr_bits > UINT32_MAX || !decimal(yuv_text,yuv) || yuv > 1)
        { error("Invalid texture display mode"); continue; }
        const uint32_t hb=uint32_t(hdr_bits);
        std::memcpy(&hdr_multiplier,&hb,sizeof(hdr_multiplier));
        if(!std::isfinite(hdr_multiplier) || (hdr_multiplier < 0.0f && hdr_multiplier != -1.0f) || (hdr_multiplier >= 0.0f && yuv))
        { error("Invalid texture display mode"); continue; }
      }
      float background[4] = {0.0f, 0.0f, 0.0f, 0.0f};
      if(arguments >> background_text)
      {
        bool valid_background = true;
        for(size_t i=0; i<4; ++i)
        {
          if(i && !(arguments >> background_text)) { valid_background=false; break; }
          uint64_t bits=0;
          if(!decimal(background_text,bits) || bits > UINT32_MAX) { valid_background=false; break; }
          const uint32_t value=uint32_t(bits);
          std::memcpy(&background[i],&value,sizeof(float));
          if(!std::isfinite(background[i]) || background[i] < 0.0f || background[i] > 1.0f) { valid_background=false; break; }
        }
        if(!valid_background) { error("Invalid texture background"); continue; }
      }
      uint64_t use_custom = 0;
      if((arguments >> custom_text) && (!decimal(custom_text,use_custom) || use_custom > 1)) { error("Invalid custom shader option"); continue; }
      if(use_custom && (custom_shader.active == ResourceId() || hdr_multiplier >= 0.0f || yuv != 0)) { error("Custom shader is unavailable or conflicts with display mode"); continue; }
      if(arguments >> extra) { error("Invalid texture display options"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      const uint64_t depth = texture.dimension == 3 ? (mip < 32 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : 1U) : texture.arraysize;
      if(mip >= texture.mips || slice >= depth || (sample_text != "resolve" && sample >= texture.samples) ||
         (channel_text != "all" && channel >= (use_custom ? 4U : texture.components)))
      { error("Texture selection is out of range"); continue; }
      float black=0.0f, white=1.0f;
      const uint32_t b=uint32_t(black_bits), w=uint32_t(white_bits);
      std::memcpy(&black,&b,sizeof(black)); std::memcpy(&white,&w,sizeof(white));
      if(!std::isfinite(black) || !std::isfinite(white) || black >= white) { error("Invalid texture display range"); continue; }
      IReplayOutput *output = replay.controller->CreateOutput(CreateHeadlessWindowingData(int32_t(width), int32_t(height)), ReplayOutputType::Texture);
      if(!output) { error("Cannot create a headless texture output"); continue; }
      TextureDisplay display;
      display.resourceId=texture.id; display.typeCast=CompType(cast);
      display.subresource=Subresource(uint32_t(mip), uint32_t(slice), sample_text == "resolve" ? TextureDisplay::ResolveSamples : uint32_t(sample));
      display.rangeMin=black; display.rangeMax=white; display.scale=scale; display.xOffset=x_offset; display.yOffset=y_offset;
      display.overlay=DebugOverlay(overlay); display.hdrMultiplier=hdr_multiplier; display.decodeYUV=yuv != 0; display.backgroundColor=FloatVector(background[0],background[1],background[2],background[3]); display.customShaderId=use_custom ? custom_shader.active : ResourceId();
      if(channel_text != "all") { display.red=channel==0; display.green=channel==1; display.blue=channel==2; display.alpha=channel==3; }
      if(has_options) { display.red=(mask & 1) != 0; display.green=(mask & 2) != 0; display.blue=(mask & 4) != 0; display.alpha=(mask & 8) != 0; display.flipY=flip != 0; display.linearDisplayAsGamma=gamma != 0; }
      output->SetTextureDisplay(display); output->Display();
      bytebuf pixels = output->ReadbackOutputTexture();
      const bool overlay_available = overlay == 0 || overlay == 7 || overlay == 8 || output->GetDebugOverlayTexID() != ResourceId();
      output->Shutdown();
      if(overlay != 0)
      {
        // Overlay replay can modify render targets and invalidate replay-derived caches.
        replay.controller->SetFrameEvent(selected_event, true);
        mesh_ready = false; history_ready = false; raw_ready = false; counters_ready = false;
        cached_history = rdcarray<PixelModification>(); raw_cache = bytebuf(); counter_results = rdcarray<CounterResult>();
      }
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      if(!overlay_available) { error("Overlay is unavailable for this event or texture"); continue; }
      if(pixels.size() != width * height * 3) { error("Unexpected texture render output size"); continue; }
      static const char hex[] = "0123456789abcdef";
      std::cout << "\nRENDERDOCX/1 {\"type\":\"texture_rendered\",\"index\":" << index << ",\"width\":" << width << ",\"height\":" << height << ",\"hex\":\"";
      for(byte value : pixels) std::cout << hex[value >> 4] << hex[value & 15];
      std::cout << "\"}" << std::endl;
      continue;
    }
    if(command.compare(0, 6, "save2 ") == 0 || command.compare(0, 6, "save3 ") == 0 || command.compare(0, 6, "save4 ") == 0 || command.compare(0, 6, "save5 ") == 0 || command.compare(0, 6, "save6 ") == 0)
    {
      const bool custom_save = command.compare(0, 6, "save6 ") == 0;
      const bool options = custom_save || command.compare(0, 6, "save5 ") == 0;
      const bool ranged = options || command.compare(0, 6, "save4 ") == 0;
      const bool extended = ranged || command.compare(0, 6, "save3 ") == 0;
      if(!selected) { error("Select an event before saving textures"); continue; }
      std::istringstream arguments(command.substr(6));
      std::string index_text, mip_text, slice_text, sample_text, channel_text, encoded, cast_text, format_text, black_text, white_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, channel = 0, cast = 0, file_type = uint64_t(FileType::PNG);
      float black = 0.0f, white = 1.0f;
      uint64_t all_mips=0, slices=0, grid_width=1, samples_array=0, alpha=3, quality=90;
      float alpha_color[4] = {0.0f, 0.0f, 0.0f, 1.0f};
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> channel_text >> encoded) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) ||
         (channel_text != "all" && !decimal(channel_text, channel)) || index >= texture_ids.size())
      { error("Invalid texture selection"); continue; }
      if(extended)
      {
        if(!(arguments >> cast_text >> format_text) || !decimal(cast_text, cast) || cast > 9 ||
           !decimal(format_text, file_type) || file_type >= uint64_t(FileType::Raw))
        { error("Invalid texture cast or file type"); continue; }
        if(ranged)
        {
          uint64_t black_bits = 0, white_bits = 0;
          if(!(arguments >> black_text >> white_text) || !decimal(black_text, black_bits) || !decimal(white_text, white_bits) || black_bits > UINT32_MAX || white_bits > UINT32_MAX)
          { error("Invalid texture display range"); continue; }
          const uint32_t b = uint32_t(black_bits), w = uint32_t(white_bits);
          std::memcpy(&black, &b, sizeof(black)); std::memcpy(&white, &w, sizeof(white));
          if(!std::isfinite(black) || !std::isfinite(white) || black >= white)
          { error("Invalid texture display range"); continue; }
        }
        if(options)
        {
          std::string mips_text, slices_text, grid_text, samples_text, alpha_text, quality_text, color_text;
          if(!(arguments >> mips_text >> slices_text >> grid_text >> samples_text >> alpha_text >> quality_text) ||
             !decimal(mips_text, all_mips) || all_mips > 1 || !decimal(slices_text, slices) || slices > 3 ||
             !decimal(grid_text, grid_width) || grid_width == 0 || grid_width > INT32_MAX ||
             !decimal(samples_text, samples_array) || samples_array > 1 || !decimal(alpha_text, alpha) || alpha > 3 ||
             !decimal(quality_text, quality) || quality < 1 || quality > 100)
          { error("Invalid texture export options"); continue; }
          bool valid_color = true;
          for(size_t i=0; i<4; ++i)
          {
            uint64_t bits=0;
            if(!(arguments >> color_text) || !decimal(color_text,bits) || bits > UINT32_MAX) { valid_color=false; break; }
            const uint32_t value=uint32_t(bits); std::memcpy(&alpha_color[i],&value,sizeof(float));
            if(!std::isfinite(alpha_color[i]) || alpha_color[i] < 0.0f || alpha_color[i] > 1.0f) { valid_color=false; break; }
          }
          if(!valid_color) { error("Invalid texture export color"); continue; }
        }
        if(arguments >> extra) { error("Invalid texture cast or file type"); continue; }
      }
      else if((arguments >> cast_text) && (!decimal(cast_text, cast) || cast > 9 || (arguments >> extra)))
      { error("Invalid texture cast"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      const uint64_t depth = texture.dimension == 3 ? (mip < 32 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : 1U) : texture.arraysize;
      if(mip >= texture.mips || mip > INT32_MAX || slice >= depth || slice > INT32_MAX ||
         (sample_text != "resolve" && sample >= texture.samples) ||
         (channel_text != "all" && (channel >= (custom_save ? 4U : texture.components) || channel > INT32_MAX)))
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
      ScopedReplayOutput scoped;
      ResourceId save_resource = texture.id;
      if(custom_save)
      {
        if(custom_shader.active == ResourceId()) { error("Custom shader is unavailable"); continue; }
        scoped.output = replay.controller->CreateOutput(CreateHeadlessWindowingData(1, 1), ReplayOutputType::Texture);
        if(!scoped.output) { error("Cannot create a headless texture output"); continue; }
        TextureDisplay display; display.resourceId=texture.id; display.typeCast=CompType(cast);
        display.subresource=Subresource(uint32_t(mip), uint32_t(slice), sample_text == "resolve" ? TextureDisplay::ResolveSamples : uint32_t(sample));
        display.customShaderId=custom_shader.active; scoped.output->SetTextureDisplay(display);
        save_resource = scoped.output->GetCustomShaderTexID();
        if(save_resource == ResourceId()) { error("Custom shader output is unavailable"); continue; }
      }
      TextureSave save;
      save.resourceId = save_resource;
      save.typeCast = custom_save ? CompType::Typeless : CompType(cast);
      save.destType = FileType(file_type);
      save.mip = all_mips ? -1 : int32_t(mip);
      save.slice.sliceIndex = slices ? -1 : int32_t(slice);
      save.slice.slicesAsGrid = slices == 2;
      save.slice.sliceGridWidth = int32_t(grid_width);
      save.slice.cubeCruciform = slices == 3;
      save.sample.mapToArray = samples_array != 0;
      save.alpha = AlphaMapping(alpha);
      save.alphaCol = FloatVector(alpha_color[0],alpha_color[1],alpha_color[2],alpha_color[3]);
      save.jpegQuality = int(quality);
      save.sample.sampleIndex = sample_text == "resolve" ? TextureSampleMapping::ResolveSamples : uint32_t(sample);
      save.channelExtract = channel_text == "all" ? -1 : int(channel);
      save.comp.blackPoint = black;
      save.comp.whitePoint = white;
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
    debug.reset();
    mesh_ready = false;
    history_ready = false;
    raw_ready = false;
    raw_cache = bytebuf();
    cached_history = rdcarray<PixelModification>();
    replay.controller->SetFrameEvent(uint32_t(event), false);
    result = replay.controller->GetFatalErrorStatus();
    if(!result.OK()) { error(result_message(result)); break; }
    selected = true;
    selected_event = uint32_t(event);
    std::cout << "\nRENDERDOCX/1 {\"type\":\"selected\",\"event_id\":" << event << ",\"pipeline\":";
    write_pipeline(replay.controller, uint32_t(event));
    std::cout << "}" << std::endl;
  }
  debug.reset();
  return 0;
}
} // namespace

int run_args(const std::vector<std::string> &args)
{
  ReplayOptions options;
  const bool worker = args.size() == 3 && args[1] == "--structure-worker";
  const bool remote = args.size() == 4 && args[1] == "--remote";
  const bool custom = args.size() == 9 && args[1] == "--open-options";
  if(args.size() != 2 && !worker && !remote && !custom) { error("Expected a capture path and optional replay options"); return 2; }
  std::string url = remote ? args[2] : (custom && args[2] != "-" ? args[2] : "");
  if((remote && url.empty()) || (custom && args[2].empty())) { error("Invalid remote server"); return 2; }
  if(custom)
  {
    uint64_t validation = 0, vendor = 0, device = 0, optimisation = 0;
    bytebuf driver;
    if(!decimal(args[3], validation) || validation > 1 || !decimal(args[4], vendor) || vendor > uint32_t(GPUVendor::Samsung) ||
       !decimal(args[5], device) || device > UINT32_MAX || !decimal(args[6], optimisation) || optimisation >= uint32_t(ReplayOptimisationLevel::Count) ||
       !decode_section_hex(args[7], driver) || std::find(driver.begin(), driver.end(), byte(0)) != driver.end())
    { error("Invalid replay options"); return 2; }
    const std::string name(driver.begin(), driver.end());
    options.apiValidation = validation != 0; options.forceGPUVendor = GPUVendor(vendor);
    options.forceGPUDeviceID = uint32_t(device); options.forceGPUDriverName = name.c_str();
    options.optimisation = ReplayOptimisationLevel(optimisation);
  }
  const std::string &capture = args[custom ? 8 : (remote ? 3 : (worker ? 2 : 1))];
  if(capture.empty()) { error("Invalid capture path"); return 2; }
  return run(capture, worker, url, options);
}

#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
{
  std::vector<std::string> args;
  for(int i = 0; i < argc; ++i)
  {
    const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[i], -1, nullptr, 0, nullptr, nullptr);
    if(size <= 0) { error("Invalid Unicode argument"); return 2; }
    std::vector<char> bytes(size_t(size), 0);
    if(!WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[i], -1, bytes.data(), size, nullptr, nullptr))
    { error("Could not convert argument"); return 2; }
    args.emplace_back(bytes.data());
  }
  return run_args(args);
}
#else
int main(int argc, char **argv) { return run_args(std::vector<std::string>(argv, argv + argc)); }
#endif
