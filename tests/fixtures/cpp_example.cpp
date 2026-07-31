// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#include <numeric>
#include <vector>

int main() {
  const std::vector<int> values{1, 2, 3};
  // breakpoint default_stop
  const int total = std::accumulate(values.begin(), values.end(), 0);
  return total == 6 ? 0 : 1;
}
