#include "api/replay/renderdoc_replay.h"
#include "api/replay/version.h"

#include <cstdint>
#include <algorithm>
#include <cstring>
#include <cwchar>
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
                    std::map<uint32_t, uint32_t> &event_chunks,
                    std::map<uint32_t, uint32_t> &draw_instances,
                    std::map<uint32_t, uint32_t> &draw_vertices,
                    std::map<uint32_t, const ActionDescription *> &dispatch_actions)
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
      if(event.chunkIndex < structured.chunks.size() && structured.chunks[event.chunkIndex])
        event_chunks.emplace(event.eventId, event.chunkIndex);
    }
    std::cout << "]}";
    event_ids.insert(action.eventId);
    if(action.flags & (ActionFlags::Drawcall | ActionFlags::MeshDispatch))
      draw_instances.emplace(action.eventId, (std::max)(1U, action.numInstances));
    if(action.flags & ActionFlags::Drawcall)
      draw_vertices.emplace(action.eventId, action.numIndices);
    if(action.flags & ActionFlags::Dispatch)
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
    std::cout << ",\"type\":" << uint32_t(resource.type) << ",\"autogenerated_name\":" << (resource.autogeneratedName ? "true" : "false")
              << ",\"initialisation_chunks\":[";
    for(size_t j = 0; j < resource.initialisationChunks.size(); ++j)
    { if(j) std::cout << ','; std::cout << resource.initialisationChunks[j]; }
    std::cout << "],\"parents\":"; resource_links(resource.parentResources);
    std::cout << ",\"derived\":"; resource_links(resource.derivedResources);
    std::cout << '}';
  }
  std::cout << "],\"textures\":[";
  for(size_t i = 0; i < textures.size(); ++i)
  {
    const TextureDescription &texture = textures[i];
    texture_ids.push_back({texture.resourceId, texture.width, texture.height, texture.dimension, texture.depth, texture.mips, texture.arraysize, texture.msSamp, texture.format.compCount});
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

void write_mesh(const MeshFormat &mesh, uint32_t event, uint32_t instance, uint32_t view, uint32_t stage)
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
  std::cout << "]}";
}

int run(const std::string &capture, bool structure_worker)
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
  DebugTraceScope debug{replay.controller};
  if(!structure_worker) { replay.file->Shutdown(); replay.file = nullptr; }
  std::set<uint32_t> event_ids;
  std::vector<TextureRef> texture_ids;
  std::vector<BufferRef> buffer_ids;
  std::map<uint32_t, uint32_t> event_chunks;
  std::map<uint32_t, uint32_t> draw_instances;
  std::map<uint32_t, uint32_t> draw_vertices;
  std::map<uint32_t, const ActionDescription *> dispatch_actions;
  write_snapshot(replay.controller, event_ids, texture_ids, buffer_ids, event_chunks, draw_instances, draw_vertices, dispatch_actions);
  std::map<ResourceId, size_t> resource_indices;
  const auto &resources = replay.controller->GetResources();
  for(size_t i = 0; i < resources.size(); ++i) resource_indices.emplace(resources[i].resourceId, i);
  bool selected = false;
  uint32_t selected_event = 0;
  bool debug_finished = false;
  bool mesh_ready = false;
  MeshFormat selected_mesh;
  uint32_t mesh_instance = 0, mesh_view = 0, mesh_stage = 0;
  bool history_ready = false;
  uint32_t history_event = 0, history_index = 0, history_mip = 0, history_slice = 0, history_sample = 0, history_x = 0, history_y = 0;
  rdcarray<PixelModification> cached_history;

  std::string command;
  while(std::getline(std::cin, command))
  {
    if(command == "quit") break;
    // A live debugger can hold backend replay state. Do not allow unrelated replay
    // operations to mutate that state between ContinueDebug calls.
    if(debug.trace && command != "debug_next" && command != "debug_stop" &&
       command.compare(0, 13, "debug_vertex ") != 0 && command.compare(0, 12, "debug_pixel ") != 0 &&
       command.compare(0, 14, "debug_compute ") != 0 &&
       command.compare(0, 7, "select ") != 0)
    { error("Stop the shader debug trace before other replay operations"); continue; }
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
      if(view != 0) { error("Nonzero debug views cannot yet be verified"); continue; }
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
      if(!selected || action == dispatch_actions.end()) { error("Select a compute dispatch event first"); continue; }
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
      // Backends can ignore or silently clamp viewID. Until the actual view count is
      // exposed by the replay API, do not report a different view as requested.
      if(view != 0) { error("Nonzero mesh views cannot yet be verified"); continue; }
      const GraphicsAPI api = replay.controller->GetAPIProperties().pipelineType;
      if(stage >= 3 && (api == GraphicsAPI::D3D11 || api == GraphicsAPI::OpenGL))
      { error("Task and mesh shader output is unavailable on this replay API"); continue; }
      mesh_ready = false;
      selected_mesh = replay.controller->GetPostVSData(uint32_t(instance), uint32_t(view), MeshDataStage(stage));
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      mesh_ready = true;
      mesh_instance = uint32_t(instance); mesh_view = uint32_t(view); mesh_stage = uint32_t(stage);
      write_mesh(selected_mesh, selected_event, uint32_t(instance), uint32_t(view), uint32_t(stage));
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
    if(command.compare(0, 17, "structure_buffer ") == 0)
    {
      if(!structure_worker || !replay.file) { error("Structured buffer export requires an isolated worker"); continue; }
      std::istringstream arguments(command.substr(17));
      std::string event_text, offset_text, length_text, item;
      uint64_t event = 0, offset = 0, length = 0;
      if(!(arguments >> event_text >> offset_text >> length_text) || !decimal(event_text, event) ||
         !decimal(offset_text, offset) || !decimal(length_text, length) || event > UINT32_MAX || length > 65536)
      { error("Invalid structured buffer request"); continue; }
      auto found = event_chunks.find(uint32_t(event));
      if(found == event_chunks.end()) { error("Structured data is unavailable for this event"); continue; }
      const SDFile &replay_data = replay.controller->GetStructuredFile();
      const SDChunk *replay_chunk = replay_data.chunks[found->second];
      const SDObject *replay_object = replay_chunk;
      std::vector<uint64_t> path;
      bool valid = true;
      while(arguments >> item)
      {
        uint64_t index = 0;
        if(!decimal(item, index) || index >= replay_object->NumChildren()) { valid = false; break; }
        replay_object = replay_object->GetChild(size_t(index));
        if(!replay_object) { valid = false; break; }
        path.push_back(index);
      }
      if(!valid || replay_object->type.basetype != SDBasic::Buffer || offset > replay_object->type.byteSize ||
         length > replay_object->type.byteSize - offset)
      { error("Invalid structured buffer path or range"); continue; }

      // The replay export normally omits buffers. This explicit request asks the capture
      // handle for its complete structured export and may use substantial memory.
      const SDFile &full = replay.file->GetStructuredData();
      if(found->second >= full.chunks.size() || !full.chunks[found->second])
      { error("Full structured capture data is unavailable"); continue; }
      const SDChunk *full_chunk = full.chunks[found->second];
      if(full.chunks.size() != replay_data.chunks.size() || full_chunk->name != replay_chunk->name ||
         full_chunk->metadata.chunkID != replay_chunk->metadata.chunkID ||
         full_chunk->metadata.length != replay_chunk->metadata.length ||
         full_chunk->metadata.threadID != replay_chunk->metadata.threadID)
      { error("Structured capture data does not match the replay event"); continue; }
      const SDObject *full_object = full_chunk;
      const SDObject *replay_compare = replay_chunk;
      for(uint64_t index : path)
      {
        if(index >= full_object->NumChildren() || index >= replay_compare->NumChildren()) { valid = false; break; }
        full_object = full_object->GetChild(size_t(index));
        replay_compare = replay_compare->GetChild(size_t(index));
        if(!full_object || !replay_compare || full_object->name != replay_compare->name ||
           full_object->type.name != replay_compare->type.name ||
           full_object->type.basetype != replay_compare->type.basetype ||
           full_object->type.byteSize != replay_compare->type.byteSize ||
           full_object->NumChildren() != replay_compare->NumChildren()) { valid = false; break; }
      }
      if(!valid || full_object->type.basetype != SDBasic::Buffer || full_object->data.basic.u >= full.buffers.size() ||
         !full.buffers[size_t(full_object->data.basic.u)] ||
         full.buffers[size_t(full_object->data.basic.u)]->size() != replay_object->type.byteSize)
      { error("Structured buffer data is unavailable or does not match the replay event"); continue; }
      const bytebuf &bytes = *full.buffers[size_t(full_object->data.basic.u)];
      static const char hex[] = "0123456789abcdef";
      std::cout << "\nRENDERDOCX/1 {\"type\":\"structured_buffer\",\"event_id\":" << event
                << ",\"offset\":\"" << offset << "\",\"path\":[";
      for(size_t i = 0; i < path.size(); ++i) { if(i) std::cout << ','; std::cout << '"' << path[i] << '"'; }
      std::cout << "],\"hex\":\"";
      for(size_t i = size_t(offset); i < size_t(offset + length); ++i)
        std::cout << hex[bytes[i] >> 4] << hex[bytes[i] & 15];
      std::cout << "\"}" << std::endl;
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
      std::cout << ",\"dispatch_threads\":";
      if(stage == ShaderStage::Compute)
        std::cout << '[' << reflection->dispatchThreadsDimension[0] << ',' << reflection->dispatchThreadsDimension[1] << ',' << reflection->dispatchThreadsDimension[2] << ']';
      else std::cout << "null";
      std::cout << ",\"debuggable\":" << (reflection->debugInfo.debuggable ? "true" : "false")
                << ",\"source_debug_info\":" << (reflection->debugInfo.sourceDebugInformation ? "true" : "false")
                << ",\"debug_status\":";
      json_string(reflection->debugInfo.debugStatus);
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
    if(command.compare(0, 8, "history ") == 0)
    {
      if(!selected) { error("Select an event before inspecting pixel history"); continue; }
      std::istringstream arguments(command.substr(8));
      std::string index_text, mip_text, slice_text, sample_text, x_text, y_text, offset_text, limit_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, x = 0, y = 0, offset = 0, limit = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> x_text >> y_text >> offset_text >> limit_text) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) || !decimal(x_text, x) || !decimal(y_text, y) ||
         !decimal(offset_text, offset) || !decimal(limit_text, limit) || index >= texture_ids.size() || limit == 0 || limit > 128)
      { error("Invalid pixel history request"); continue; }
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
         history_sample != actual_sample || history_x != x || history_y != y)
      {
        history_ready = false;
        cached_history = rdcarray<PixelModification>();
        cached_history = replay.controller->PixelHistory(texture.id, uint32_t(x), uint32_t(y), Subresource(uint32_t(mip), uint32_t(slice), actual_sample), CompType::Typeless);
        result = replay.controller->GetFatalErrorStatus();
        if(!result.OK()) { error(result_message(result)); break; }
        history_event = selected_event; history_index = uint32_t(index); history_mip = uint32_t(mip); history_slice = uint32_t(slice);
        history_sample = actual_sample; history_x = uint32_t(x); history_y = uint32_t(y); history_ready = true;
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
    if(command.compare(0, 6, "stats ") == 0)
    {
      if(!selected) { error("Select an event before computing texture statistics"); continue; }
      std::istringstream arguments(command.substr(6));
      std::string index_text, mip_text, slice_text, sample_text, min_text, max_text, mask_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, min_bits = 0, max_bits = 0, mask = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> min_text >> max_text >> mask_text) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) || !decimal(min_text, min_bits) || !decimal(max_text, max_bits) || !decimal(mask_text, mask) ||
         index >= texture_ids.size() || min_bits > UINT32_MAX || max_bits > UINT32_MAX || mask > 15)
      { error("Invalid texture statistics request"); continue; }
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
      const Subresource sub(uint32_t(mip), uint32_t(slice), actual_sample);
      const auto extrema = replay.controller->GetMinMax(texture.id, sub, CompType::Typeless);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      const rdcfixedarray<bool, 4> channels = {(mask & 1) != 0, (mask & 2) != 0, (mask & 4) != 0, (mask & 8) != 0};
      const rdcarray<uint32_t> histogram = replay.controller->GetHistogram(texture.id, sub, CompType::Typeless, lo, hi, channels);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      uint32_t low[4], high[4]; std::memcpy(low, &extrema.first, sizeof(low)); std::memcpy(high, &extrema.second, sizeof(high));
      std::cout << "\nRENDERDOCX/1 {\"type\":\"texture_stats\",\"event_id\":" << selected_event << ",\"index\":" << index
                << ",\"mip\":" << mip << ",\"slice\":" << slice << ",\"sample\":" << actual_sample
                << ",\"range_min_bits\":" << lo_bits << ",\"range_max_bits\":" << hi_bits << ",\"channel_mask\":" << mask
                << ",\"min_bits\":[" << low[0] << ',' << low[1] << ',' << low[2] << ',' << low[3] << "],\"max_bits\":[" << high[0] << ',' << high[1] << ',' << high[2] << ',' << high[3] << "],\"histogram\":[";
      for(size_t i = 0; i < histogram.size(); ++i) { if(i) std::cout << ','; std::cout << histogram[i]; }
      std::cout << "]}" << std::endl;
      continue;
    }
    if(command.compare(0, 6, "pixel ") == 0)
    {
      if(!selected) { error("Select an event before picking a pixel"); continue; }
      std::istringstream arguments(command.substr(6));
      std::string index_text, mip_text, slice_text, sample_text, x_text, y_text, extra;
      uint64_t index = 0, mip = 0, slice = 0, sample = 0, x = 0, y = 0;
      if(!(arguments >> index_text >> mip_text >> slice_text >> sample_text >> x_text >> y_text) || (arguments >> extra) ||
         !decimal(index_text, index) || !decimal(mip_text, mip) || !decimal(slice_text, slice) ||
         (sample_text != "resolve" && !decimal(sample_text, sample)) || !decimal(x_text, x) || !decimal(y_text, y) || index >= texture_ids.size())
      { error("Invalid pixel request"); continue; }
      const TextureRef &texture = texture_ids[size_t(index)];
      if(mip >= texture.mips || mip >= 32 || !texture.width || !texture.height || !texture.samples)
      { error("Invalid texture dimensions, mip or sample count"); continue; }
      const uint32_t width = (std::max)(1U, texture.width >> uint32_t(mip));
      const uint32_t height = (std::max)(1U, texture.height >> uint32_t(mip));
      const uint32_t depth = texture.dimension == 3 ? (std::max)(1U, texture.depth >> uint32_t(mip)) : texture.arraysize;
      if(x >= width || y >= height || slice >= depth || (sample_text != "resolve" && sample >= texture.samples))
      { error("Pixel selection is out of range"); continue; }
      const uint32_t actual_sample = sample_text == "resolve" ? (texture.samples > 1 ? UINT32_MAX : 0U) : uint32_t(sample);
      PixelValue pixel = replay.controller->PickPixel(texture.id, uint32_t(x), uint32_t(y), Subresource(uint32_t(mip), uint32_t(slice), actual_sample), CompType::Typeless);
      result = replay.controller->GetFatalErrorStatus();
      if(!result.OK()) { error(result_message(result)); break; }
      uint32_t bits[4]; std::memcpy(bits, &pixel, sizeof(bits));
      std::cout << "\nRENDERDOCX/1 {\"type\":\"pixel\",\"event_id\":" << selected_event << ",\"index\":" << index
                << ",\"mip\":" << mip << ",\"slice\":" << slice << ",\"sample\":" << actual_sample
                << ",\"x\":" << x << ",\"y\":" << y << ",\"bits\":[" << bits[0] << ',' << bits[1] << ',' << bits[2] << ',' << bits[3] << "]}" << std::endl;
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
    debug.reset();
    mesh_ready = false;
    history_ready = false;
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
  return 0;
}
} // namespace

#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
{
  const bool worker = argc == 3 && std::wcscmp(argv[1], L"--structure-worker") == 0;
  if(argc != 2 && !worker) { error("Expected one capture file path"); return 2; }
  const wchar_t *capture = argv[worker ? 2 : 1];
  const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, capture, -1, nullptr, 0, nullptr, nullptr);
  if(size <= 0) { error("Capture path is not valid Unicode"); return 2; }
  std::vector<char> path(size_t(size), 0);
  if(WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, capture, -1, path.data(), size, nullptr, nullptr) != size)
  { error("Could not convert capture path"); return 2; }
  return run(path.data(), worker);
}
#else
int main(int argc, char **argv)
{
  const bool worker = argc == 3 && std::strcmp(argv[1], "--structure-worker") == 0;
  if(argc != 2 && !worker) { error("Expected one capture file path"); return 2; }
  return run(argv[worker ? 2 : 1], worker);
}
#endif
