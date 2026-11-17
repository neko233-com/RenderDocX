#include "../../bridge/replay/descriptor_batch.h"
#include <cstdlib>
#include <optional>
#include <vector>

int main()
{
  auto check = [](bool value) { if(!value) std::abort(); };
  for(bool incomplete : {false, true})
  {
    size_t singles = 0;
    std::vector<std::optional<int>> rows(3);
    distribute_descriptor_batch(3,
      [&] { return incomplete ? std::vector<int>{20, 30} : std::vector<int>{10, 20, 30}; },
      [&](size_t i) { ++singles; return i == 1 ? std::vector<int>{} : std::vector<int>{int(10 * (i + 1))}; },
      [&](size_t i, int value) { rows[i] = value; });
    check(rows[0] == 10 && rows[2] == 30);
    check(incomplete ? !rows[1].has_value() : rows[1] == 20);
    check(singles == (incomplete ? 3U : 0U));
  }
  bool called = false;
  distribute_descriptor_batch(0, [&] { called = true; return std::vector<int>{}; },
    [](size_t) { return std::vector<int>{}; }, [](size_t, int) {});
  check(!called);
}
