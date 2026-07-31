#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.
# pyre-strict


def main() -> int:
    message = "hello world"
    print(message)

    numbers = [1, 2, 3, 4, 5]
    total = 0
    # breakpoint default_stop
    for num in numbers:
        # breakpoint secondary_stop
        total += num

    # breakpoint tertiary_stop
    print(f"Sum: {total}")
    # breakpoint quaternary_stop
    return total


if __name__ == "__main__":
    main()
