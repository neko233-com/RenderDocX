#include <filesystem>
#include <cstdio>
#include <fstream>
#include <iostream>
#include <string>
#include <chrono>
#include <thread>
#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

template <typename Char> int run(int argc, Char **argv)
{
  if(argc == 2 && std::filesystem::path(argv[1]) == std::filesystem::path("--sleep"))
  {
    std::this_thread::sleep_for(std::chrono::seconds(2));
    return 0;
  }
  if(argc == 2 && std::filesystem::path(argv[1]) == std::filesystem::path("--fail"))
  {
    std::cerr << "fixture compiler failed";
    return 7;
  }
  if(argc == 1)
  {
    std::cout << std::cin.rdbuf();
    std::cerr << "fixture compiler diagnostics";
    return std::cin.bad() || std::cout.bad() ? 1 : 0;
  }
  if(argc != 3) return 2;
  std::ifstream input(std::filesystem::path(argv[1]), std::ios::binary);
  std::ofstream output(std::filesystem::path(argv[2]), std::ios::binary);
  if(!input || !output) return 3;
  output << input.rdbuf();
  std::cerr << "fixture compiler diagnostics";
  return input.bad() || output.bad() ? 4 : 0;
}

#ifdef _WIN32
int wmain(int argc, wchar_t **argv)
{
  _setmode(_fileno(stdin), _O_BINARY);
  _setmode(_fileno(stdout), _O_BINARY);
  return run(argc, argv);
}
#else
int main(int argc, char **argv) { return run(argc, argv); }
#endif
