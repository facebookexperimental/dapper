// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#include <numeric>
#include <vector>

int sum_values(const std::vector<int>& values) {
  // breakpoint default_stop
  return std::accumulate(values.begin(), values.end(), 0);
}

int main() {
  const std::vector<int> values{1, 2, 3};
  const int total = sum_values(values);
  return total == 6 ? 0 : 1;
}
