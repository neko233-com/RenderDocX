#pragma once

#include <cstddef>

// Single-range queries are the only safe recovery when a batch is incomplete:
// the missing element's position is not known.
template <typename Batch, typename Single, typename Assign>
void distribute_descriptor_batch(size_t count, Batch batch, Single single, Assign assign)
{
  if(count == 0) return;
  const auto values = batch();
  if(values.size() == count)
  {
    for(size_t i = 0; i < count; ++i) assign(i, values[i]);
  }
  else
  {
    for(size_t i = 0; i < count; ++i)
    {
      const auto value = single(i);
      if(!value.empty()) assign(i, value[0]);
    }
  }
}
