// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#include <vector>

int sum_values(const std::vector<int>& values) {
  int total = 0;
  for (const int value : values) {
    // breakpoint default_stop
    total += value;
  }
  return total;
}

int main() {
  const std::vector<int> values{1, 2, 3};
  const int total = sum_values(values);
  return total == 6 ? 0 : 1;
}
