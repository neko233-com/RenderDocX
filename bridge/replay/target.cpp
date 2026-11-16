#include "api/replay/renderdoc_replay.h"
#include "api/replay/version.h"
#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <memory>
#include <mutex>
#include <queue>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif

REPLAY_PROGRAM_MARKER();

namespace
{
void json(const char *s, size_t len)
{
  static const char hex[] = "0123456789abcdef";
  std::cout << '"';
  for(size_t i = 0; i < len; ++i)
  {
    unsigned char c = static_cast<unsigned char>(s[i]);
    if(c == '"' || c == '\\') std::cout << '\\' << char(c);
    else if(c < 32) std::cout << "\\u00" << hex[c >> 4] << hex[c & 15];
    else std::cout << char(c);
  }
  std::cout << '"';
}
void json(const rdcstr &s) { json(s.c_str(), s.size()); }
void failure(const std::string &s)
{
  std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"error\",\"message\":";
  json(s.data(), s.size());
  std::cout << "}" << std::endl;
}
std::string detail(const ResultDetails &r)
{
  return r.internal_msg ? std::string(r.internal_msg->c_str(), r.internal_msg->size()) : "RenderDoc error " + std::to_string(uint32_t(r.code));
}
bool decimal(const std::string &s, uint32_t &v)
{
  uint64_t n = 0;
  if(s.empty()) return false;
  for(char c : s)
  {
    if(c < '0' || c > '9') return false;
    n = n * 10 + uint64_t(c - '0');
    if(n > (std::numeric_limits<uint32_t>::max)()) return false;
  }
  v = uint32_t(n); return true;
}
struct Commands { std::mutex mutex; std::queue<std::string> values; };

int run(const std::vector<std::string> &args)
{
  if(args.size() != 2 && args.size() != 3 && args.size() != 5) { failure("Expected --enumerate, --connect IDENT or --launch APP TEMPLATE ARGS"); return 2; }
  const char *version = RENDERDOC_GetVersionString(), *commit = RENDERDOC_GetCommitHash();
  if(!version || !commit || std::strcmp(version, MAJOR_MINOR_VERSION_STRING) || std::strcmp(commit, RENDERDOCX_CORE_COMMIT))
  { failure("RenderDoc core does not match the version and commit used to build this bridge"); return 1; }
  GlobalEnvironment env;
  RENDERDOC_InitialiseReplay(env, {});
  if(args.size() == 2 && args[1] == "--enumerate")
  {
    std::vector<uint32_t> idents;
    uint32_t ident = 0;
    while((ident = RENDERDOC_EnumerateRemoteTargets("", ident)) != 0) idents.push_back(ident);
    std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"targets\",\"idents\":[";
    for(size_t i = 0; i < idents.size(); ++i) { if(i) std::cout << ','; std::cout << idents[i]; }
    std::cout << "]}" << std::endl;
    RENDERDOC_ShutdownReplay(); return 0;
  }
  uint32_t ident = 0;
  if(args[1] == "--launch" && args.size() == 5)
  {
    ExecuteResult result = RENDERDOC_ExecuteAndInject(args[2].c_str(), "", args[4].c_str(), {}, args[3].c_str(), CaptureOptions(), false);
    if(!result.result.OK() || !result.ident)
    { failure(detail(result.result)); RENDERDOC_ShutdownReplay(); return 1; }
    ident = result.ident;
  }
  else if(args[1] == "--connect" && args.size() == 3 && decimal(args[2], ident) && ident != 0) {}
  else { failure("Invalid target arguments"); RENDERDOC_ShutdownReplay(); return 2; }
  ITargetControl *target = RENDERDOC_CreateTargetControl("", ident, "RenderDocX", false);
  if(!target || !target->Connected() || !target->GetBusyClient().empty())
  {
    rdcstr busy = target ? target->GetBusyClient() : rdcstr();
    const std::string message = "Could not connect to target " + std::to_string(ident) + (busy.empty() ? "" : "; busy: " + std::string(busy.c_str(), busy.size()));
    if(target) target->Shutdown();
    failure(message); RENDERDOC_ShutdownReplay(); return 1;
  }
  std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"connected\",\"ident\":" << ident << ",\"pid\":" << target->GetPID() << ",\"target\":";
  json(target->GetTarget()); std::cout << ",\"api\":"; json(target->GetAPI()); std::cout << "}" << std::endl;
  auto commands = std::make_shared<Commands>();
  std::thread([commands]() {
    std::string line;
    while(std::getline(std::cin, line)) { std::lock_guard<std::mutex> lock(commands->mutex); commands->values.push(line); }
    std::lock_guard<std::mutex> lock(commands->mutex); commands->values.push("quit");
  }).detach();
  bool quit = false;
  while(!quit && target->Connected())
  {
    std::queue<std::string> pending;
    { std::lock_guard<std::mutex> lock(commands->mutex); pending.swap(commands->values); }
    while(!pending.empty())
    {
      std::string line = pending.front(); pending.pop();
      if(line == "quit") { quit = true; break; }
      std::istringstream parts(line);
      std::string command, a, b, extra;
      parts >> command;
      uint32_t first = 0, count = 0;
      if(command == "trigger" && (parts >> a) && !(parts >> extra) && decimal(a, count) && count > 0)
        target->TriggerCapture(count);
      else if(command == "queue" && (parts >> a >> b) && !(parts >> extra) && decimal(a, first) && decimal(b, count) && count > 0)
        target->QueueCapture(first, count);
      else if(command == "cycle" && !(parts >> extra)) target->CycleActiveWindow();
      else { failure("Invalid target command"); continue; }
      if(!target->Connected()) { failure("Target disconnected while sending command"); quit = true; break; }
      std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"command_sent\",\"command\":";
      json(command.data(), command.size()); std::cout << "}" << std::endl;
    }
    if(quit) break;
    TargetControlMessage msg = target->ReceiveMessage();
    switch(msg.type)
    {
      case TargetControlMessageType::Noop: break;
      case TargetControlMessageType::NewCapture:
      case TargetControlMessageType::CaptureCopied:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"" << (msg.type == TargetControlMessageType::NewCapture ? "capture" : "copied")
                  << "\",\"capture_id\":" << msg.newCapture.captureId << ",\"frame\":" << msg.newCapture.frameNumber
                  << ",\"timestamp\":\"" << msg.newCapture.timestamp << "\",\"bytes\":\"" << msg.newCapture.byteSize
                  << "\",\"local\":" << (msg.newCapture.local ? "true" : "false") << ",\"path\":";
        json(msg.newCapture.path); std::cout << ",\"title\":"; json(msg.newCapture.title); std::cout << ",\"api\":"; json(msg.newCapture.api); std::cout << "}" << std::endl; break;
      case TargetControlMessageType::RegisterAPI:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"api\",\"name\":"; json(msg.apiUse.name);
        std::cout << ",\"presenting\":" << (msg.apiUse.presenting ? "true" : "false") << ",\"supported\":" << (msg.apiUse.supported ? "true" : "false") << ",\"message\":";
        json(msg.apiUse.supportMessage); std::cout << "}" << std::endl; break;
      case TargetControlMessageType::CaptureProgress:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"progress\",\"value\":";
        if(std::isfinite(msg.capProgress)) std::cout << msg.capProgress; else std::cout << "null";
        std::cout << "}" << std::endl; break;
      case TargetControlMessageType::NewChild:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"child\",\"pid\":" << msg.newChild.processId << ",\"ident\":" << msg.newChild.ident << "}" << std::endl; break;
      case TargetControlMessageType::CapturableWindowCount:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"windows\",\"count\":" << msg.capturableWindowCount << "}" << std::endl; break;
      case TargetControlMessageType::Busy:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"busy\",\"client\":"; json(msg.busy.clientName); std::cout << "}" << std::endl; break;
      case TargetControlMessageType::RequestShow:
        std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"request_show\"}" << std::endl; break;
      case TargetControlMessageType::Disconnected: quit = true; break;
      default: break;
    }
  }
  target->Shutdown();
  RENDERDOC_ShutdownReplay();
  std::cout << "\nRENDERDOCX-TARGET/1 {\"type\":\"disconnected\"}" << std::endl;
  return 0;
}
}

#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
{
  std::vector<std::string> args;
  for(int i = 0; i < argc; ++i)
  {
    int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[i], -1, nullptr, 0, nullptr, nullptr);
    if(size <= 0) { failure("Invalid Unicode argument"); return 2; }
    std::vector<char> bytes(size_t(size), 0);
    if(!WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, argv[i], -1, bytes.data(), size, nullptr, nullptr)) { failure("Invalid Unicode argument"); return 2; }
    args.emplace_back(bytes.data());
  }
  return run(args);
}
#else
int main(int argc, char **argv) { return run(std::vector<std::string>(argv, argv + argc)); }
#endif
