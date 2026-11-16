#include "api/replay/renderdoc_replay.h"
#include <cstdlib>

REPLAY_PROGRAM_MARKER();

int main(int argc, char **argv)
{
  if(argc != 2) return 2;
  const unsigned long port = std::strtoul(argv[1], nullptr, 10);
  if(port == 0 || port > 65535) return 2;
  GlobalEnvironment env;
  RENDERDOC_InitialiseReplay(env, {});
  RENDERDOC_BecomeRemoteServer("127.0.0.1", uint16_t(port));
  RENDERDOC_ShutdownReplay();
}
